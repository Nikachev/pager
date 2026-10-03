// DOM control and operation scheduling, independent from the USB transport.
class PagerApp {
  constructor(doc, usb, options = {}) {
    this.doc = doc;
    this.usb = usb;
    this.state = null;
    this.gps = null;
    this.gpsZone = null;
    this.gpsRefreshing = null;
    this.gpsTimer = null;
    this.gpsReceived = 0;
    this.gpsError = null;
    this.busy = false;
    this.refreshing = null;
    this.refreshAgain = false;
    this.refreshTimer = null;
    this.revision = null;
    this.epoch = 0;
    this.renamingSlot = null;
    this.confirmPending = false;
    this.session = new PagerSession({
      ...options,
      onEvent: bytes => this.event(bytes),
      onDisconnect: error => this.disconnected(error)
    });
    this.bind();
    this.render();
  }

  element(id) { return this.doc.getElementById(id); }

  log(message, kind = '') {
    const root = this.element('log');
    const row = this.doc.createElement('div');
    row.className = kind;
    // Limit both rows and one large GET_LOGS result. Never log commands or text.
    row.textContent = `${new Date().toLocaleTimeString()}  ${String(message).slice(0, 4096)}`;
    root.append(row);
    while (root.childElementCount > 200) root.firstElementChild.remove();
    root.scrollTop = root.scrollHeight;
  }

  disconnected(error) {
    ++this.epoch;
    clearTimeout(this.gpsTimer);
    this.gpsTimer = null;
    this.gps = null;
    this.gpsZone = null;
    this.gpsRefreshing = null;
    this.gpsError = null;
    clearTimeout(this.refreshTimer);
    this.refreshTimer = null;
    this.refreshAgain = false;
    this.refreshing = null;
    this.revision = null;
    this.state = null;
    this.renamingSlot = null;
    for (const id of ['rename_dialog', 'slot_name_dialog', 'confirm_dialog']) {
      if (this.element(id).open) this.element(id).close('cancel');
    }
    this.render();
    if (error) this.log(error.message, 'err');
  }

  async disconnect() {
    const cleanup = this.session.disconnect();
    this.disconnected();
    await cleanup;
    this.log('Disconnected from Pager', 'warn');
  }

  event(bytes) {
    const revision = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(1, true);
    const gap = this.revision !== null && revision !== ((this.revision + 1) >>> 0);
    this.revision = revision;
    if (this.refreshTimer !== null && bytes[0] !== 255 && !gap) return;
    clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(() => {
      this.refreshTimer = null;
      this.refresh().catch(() => {});
    }, bytes[0] === 255 || gap ? 0 : 25);
  }

  refresh() {
    if (!this.session.connected) return Promise.resolve();
    if (this.busy && !this.refreshing) {
      this.refreshAgain = true;
      return Promise.resolve();
    }
    if (this.refreshing) {
      this.refreshAgain = true;
      return this.refreshing;
    }
    const epoch = this.epoch;
    this.refreshing = (async () => {
      try {
        do {
          this.refreshAgain = false;
          const bytes = await this.session.call([PAGER_PROTOCOL.commands.get_state]);
          if (epoch !== this.epoch) return;
          this.state = PagerCodec.decodeState(bytes);
          this.render();
        } while (this.refreshAgain && !this.busy && this.session.connected);
      } catch (error) {
        if (epoch === this.epoch) {
          this.disconnected(error);
          await this.session.disconnect();
        }
        throw error;
      } finally {
        if (epoch === this.epoch) this.refreshing = null;
      }
    })();
    return this.refreshing;
  }

  async operate(label, operation, {refresh = true} = {}) {
    if (this.busy) return false;
    this.busy = true;
    const epoch = this.epoch;
    this.render();
    this.log(label);
    let success = false;
    try {
      if (this.refreshing) await this.refreshing;
      if (this.gpsRefreshing) await this.gpsRefreshing;
      await operation();
      success = epoch === this.epoch;
      if (success) this.log('Done', 'ok');
    } catch (error) {
      this.log(error.message, 'err');
    } finally {
      this.busy = false;
      this.render();
      if (this.session.connected && (refresh || this.refreshAgain)) {
        await this.refresh().catch(() => {});
      }
    }
    return success;
  }

  async command(payload) {
    const response = await this.session.call(payload);
    if (response.length !== 1 || response[0] !== 0) throw Error('Invalid command acknowledgement');
    return response;
  }

  nameBytes(value, limit, label) {
    const text = value.trim();
    const bytes = new TextEncoder().encode(text);
    if (!text) throw Error(`${label} cannot be empty`);
    if (bytes.length > limit) throw Error(`${label} must be at most ${limit} UTF-8 bytes`);
    return bytes;
  }

  async confirm(title, message) {
    if (this.confirmPending || this.busy || !this.session.connected) return false;
    this.confirmPending = true;
    const dialog = this.element('confirm_dialog');
    const previous = this.doc.activeElement;
    this.element('confirm_title').textContent = title;
    this.element('confirm_message').textContent = message;
    dialog.returnValue = 'cancel';
    return new Promise(resolve => {
      dialog.addEventListener('close', () => {
        this.confirmPending = false;
        previous?.focus();
        resolve(dialog.returnValue === 'default' && this.session.connected);
      }, {once: true});
      dialog.showModal();
      dialog.querySelector('button[value="cancel"]').focus();
    });
  }

  openName(dialogId, inputId, value) {
    if (this.busy || !this.session.connected) return;
    const previous = this.doc.activeElement;
    const dialog = this.element(dialogId);
    dialog.returnValue = 'cancel';
    this.element(inputId).value = value;
    dialog.addEventListener('close', () => previous?.focus(), {once: true});
    dialog.showModal();
    this.element(inputId).focus();
  }

  render() {
    const connected = this.session.connected;
    const state = this.state;
    const ready = connected && !!state && !this.busy;
    this.element('usb_status').textContent = this.busy ? 'Working…' : connected ? 'USB connected' : 'USB disconnected';
    this.element('usb_status').className = 'pill' + (this.busy ? ' busy' : connected ? ' on' : '');
    this.element('connect').hidden = connected;
    this.element('connect').disabled = this.busy || !this.usb;
    this.element('disconnect').hidden = !connected;
    this.element('disconnect').disabled = this.busy;
    this.element('device_hint').textContent = !this.usb ? 'WebUSB requires a compatible browser such as Chrome.' : !state ? 'Connect Pager to manage Bluetooth.' : !state.enabled ? `${state.baseName} Bluetooth radio is off.` : state.active === null ? `${state.baseName} is on. Choose a slot.` : `${state.baseName} ${state.active + 1} is active.`;
    const links = ['Bluetooth off','Ready','Advertising','Waiting for pairing','Connecting','Connected','Disconnecting'];
    this.element('ble_status').textContent = !state ? 'Bluetooth unavailable' : state.link === 5 && !state.hidReady ? 'Connected · waiting for keyboard subscription' : links[state.link];
    this.element('ble_status').className = 'pill' + (state?.hidReady ? ' on' : [3,4,5,6].includes(state?.link) ? ' busy' : '');
    for (const id of ['bluetooth_toggle','rename_device','cancel_pairing']) {
      this.element(id).hidden = !connected || (id === 'cancel_pairing' && !state?.pairing);
      this.element(id).disabled = !ready;
    }
    this.element('bluetooth_toggle').textContent = state?.enabled ? 'Turn Bluetooth off' : 'Turn Bluetooth on';
    for (const id of ['get_logs','reboot','factory_reset']) this.element(id).disabled = !ready;
    for (const id of ['text','type']) this.element(id).disabled = !ready || !state.hidReady;
    this.renderSlots(ready);
    this.renderGps();
  }

  async refreshGps() {
    if (!this.session.connected || this.gpsRefreshing) return;
    const epoch = this.epoch;
    this.gpsRefreshing = (async () => {
      try {
        const bytes = await this.session.call([PAGER_PROTOCOL.commands.get_gps]);
        if (epoch !== this.epoch) return;
        this.gps = PagerGps.decode(bytes);
        this.gpsReceived = performance.now();
        this.gpsError = null;
        if (this.gps.position || this.gps.lastPosition) {
          try { this.gpsZone = PagerGps.zone(this.gps.position || this.gps.lastPosition); }
          catch (_) { this.gpsZone = null; }
        }
        this.renderGps();
      } catch (error) {
        if (epoch !== this.epoch) return;
        this.gps = null;
        this.gpsError = error.message;
        this.renderGps();
      } finally {
        if (epoch === this.epoch) this.gpsRefreshing = null;
      }
    })();
    return this.gpsRefreshing;
  }

  scheduleGps() {
    clearTimeout(this.gpsTimer);
    if (!this.session.connected || this.gps?.supported === false || this.gpsError) return;
    this.gpsTimer = setTimeout(async () => {
      this.gpsTimer = null;
      this.renderGps();
      if (!this.busy && !this.refreshing) await this.refreshGps();
      this.scheduleGps();
    }, 1000);
    this.gpsTimer?.unref?.();
  }

  renderGps() {
    const gps = this.session.connected ? this.gps : null;
    const elapsed = gps ? Math.max(0, performance.now() - this.gpsReceived) : 0;
    const fresh = elapsed < 5000;
    const position = fresh ? gps?.position : null;
    const last = fresh && !position ? gps?.lastPosition : null;
    const displayed = position || last;
    const status = this.element('gps_status');
    status.textContent = !gps ? 'GPS unavailable' : !gps.supported ? 'GPS not supported' : !fresh ? 'GPS data stale' : position ? 'GPS fix' : gps.connected ? 'Searching for satellites' : 'GPS not detected';
    status.className = 'pill' + (position ? ' on' : gps?.connected ? ' busy' : '');
    this.element('gps_latitude').textContent = displayed ? `${displayed.lat.toFixed(6)}°` : '—';
    this.element('gps_longitude').textContent = displayed ? `${displayed.lon.toFixed(6)}°` : '—';
    this.element('copy_coordinates').disabled = !position || !this.doc.defaultView?.navigator?.clipboard;
    if (!position) this.element('copy_coordinates_status').textContent = '';
    this.element('gps_hint').textContent = this.gpsError ? 'Update firmware to read GPS and board time.' : !gps ? 'Connect XIAO to read GPS.' : !gps.supported ? 'L76K is supported on XIAO boards.' : position ? `${gps.satellites} satellites in use` : last ? `Last known location · ${Math.floor((last.age + elapsed) / 1000)} s ago. Waiting for a new GPS fix.` : 'Waiting for a location fix. Place the antenna with a clear view of the sky.';
    const zone = this.gpsZone || 'UTC';
    this.element('board_timezone').textContent = this.gpsZone ? `${zone} · estimated from ${position ? 'GPS location' : 'last GPS location'}` : 'UTC · waiting for location';
    this.element('board_time').textContent = gps?.utcMs != null && fresh ? PagerGps.time(gps.utcMs + elapsed, zone) : gps?.utcMs != null ? 'Board time unavailable' : 'Waiting for GPS time';
    const age = gps?.age == null ? null : gps.age + elapsed;
    this.element('board_time_source').textContent = !gps?.supported ? 'GPS time unavailable.' : gps.utcMs == null ? 'Time has not been synchronized.' : !fresh ? 'USB clock sample is stale.' : age < 5000 ? 'Synchronized with GPS · local time includes daylight saving.' : `Board clock running · last GPS correction ${Math.floor(age / 1000)} s ago.`;
  }

  renderSlots(ready) {
    const root = this.element('slots');
    const focused = this.doc.activeElement?.id;
    root.replaceChildren();
    for (let slot = 0; slot < PAGER_PROTOCOL.limits.slots; slot++) {
      const state = this.state;
      const bonded = !!state?.bonds[slot];
      const active = state?.active === slot;
      const connected = state?.connected === slot;
      const card = this.doc.createElement('article');
      card.className = 'slot' + (active ? ' active' : '') + (connected ? ' connected' : '') + (active && state?.pairing ? ' pairing' : '') + (!bonded ? ' empty' : '');
      const add = (className, text) => {
        const node = this.doc.createElement('div'); node.className = className;
        node.textContent = text; card.append(node); return node;
      };
      add('slot-number', `${state?.baseName || 'Pager'} ${slot + 1}`);
      const status = !state ? 'Unavailable' : !state.enabled ? 'Bluetooth off' : connected ? 'Connected' : active && state.pairing ? 'Waiting for pairing' : active && state.link === 4 ? 'Connecting' : bonded ? 'Saved' : 'Empty';
      add('slot-state', status);
      const name = bonded ? state.names[slot] || state.addresses[slot] || 'Unknown device' : 'Empty slot';
      add('slot-name', name).title = name;
      add('slot-address', bonded ? state.addresses[slot] || 'Unknown address' : '');
      const actions = add('slot-actions', '');
      const button = (suffix, text, label, handler, disabled = false) => {
        const node = this.doc.createElement('button');
        node.id = `slot_${slot}_${suffix}`; node.className = 'btn';
        node.textContent = text; node.setAttribute('aria-label', label);
        node.disabled = !ready || disabled; node.onclick = handler; actions.append(node);
      };
      button('activate', bonded ? active ? 'Active' : 'Connect' : 'Pair', `${bonded ? 'Connect' : 'Pair'} slot ${slot + 1}`,
        () => this.operate(`Activating slot ${slot + 1}`, () => this.command([PAGER_PROTOCOL.commands.activate_slot, slot])), !state?.enabled || (active && bonded));
      if (bonded) {
        button('rename', '✎', `Rename slot ${slot + 1}`, () => {
          this.renamingSlot = slot;
          this.openName('slot_name_dialog','slot_name_input', state.names[slot] || state.addresses[slot]);
        });
        button('clear', '×', `Free slot ${slot + 1}`, async () => {
          if (await this.confirm(`Free slot ${slot + 1}`, 'Delete the saved Bluetooth bond and alias?')) {
            await this.operate(`Clearing slot ${slot + 1}`, () => this.command([PAGER_PROTOCOL.commands.clear_slot, slot]));
          }
        });
      }
      root.append(card);
    }
    if (focused?.startsWith('slot_')) this.element(focused)?.focus();
  }

  bind() {
    const cmd = PAGER_PROTOCOL.commands;
    this.element('copy_coordinates').onclick = async () => {
      const position = this.session.connected && performance.now() - this.gpsReceived < 5000 ? this.gps?.position : null;
      const clipboard = this.doc.defaultView?.navigator?.clipboard;
      if (!position || !clipboard) return;
      const epoch = this.epoch;
      try {
        await clipboard.writeText(`${position.lat.toFixed(6)}, ${position.lon.toFixed(6)}`);
        if (epoch === this.epoch) this.element('copy_coordinates_status').textContent = 'Copied · paste into Google Maps search';
      } catch (_) {
        if (epoch === this.epoch) this.element('copy_coordinates_status').textContent = 'Could not copy. Select the coordinates to copy manually.';
      }
    };
    this.element('connect').onclick = () => {
      if (this.busy) return;
      ++this.epoch;
      this.refreshing = null;
      return this.operate('Connecting to Pager', async () => {
        const device = await this.usb.requestDevice({
          filters: [{vendorId: PAGER_PROTOCOL.application_vid, productId: PAGER_PROTOCOL.application_pid}]
        });
        try {
          await this.session.connect(device);
          // Initial state belongs to this connection, before exposing controls.
          this.state = PagerCodec.decodeState(await this.session.call([cmd.get_state]));
          await this.refreshGps();
          this.scheduleGps();
        } catch (error) {
          await this.session.disconnect();
          this.disconnected();
          throw error;
        }
      });
    };
    this.element('disconnect').onclick = () => this.operate('Disconnecting', () => this.disconnect(), {refresh: false});
    this.usb?.addEventListener('disconnect', event => {
      if (event.device === this.session.session?.device) this.disconnect();
    });
    this.element('bluetooth_toggle').onclick = () => {
      const enabled = this.state.enabled;
      return this.operate(enabled ? 'Turning Bluetooth off' : 'Turning Bluetooth on', () => this.command([cmd.set_bluetooth_enabled, enabled ? 0 : 1]));
    };
    this.element('cancel_pairing').onclick = () => this.operate('Cancelling pairing', () => this.command([cmd.cancel_pairing]));
    this.element('type').onclick = async () => {
      const bytes = new TextEncoder().encode(this.element('text').value);
      if (!bytes.length) return;
      if (bytes.length > PAGER_PROTOCOL.limits.type_text) { this.log('Text must be at most 256 UTF-8 bytes', 'err'); return; }
      if (await this.operate('Sending keyboard text', () => this.command([cmd.type_text, ...bytes]))) this.element('text').value = '';
    };
    this.element('get_logs').onclick = () => this.operate('Reading device log', async () => {
      const bytes = await this.session.call([cmd.get_logs]);
      this.log(new TextDecoder().decode(bytes) || 'Device log is empty', 'ok');
    }, {refresh: false});
    for (const [id, title, message, command] of [
      ['reboot','Reboot to bootloader','USB will disconnect while Pager enters firmware update mode.',cmd.reboot_to_bootloader],
      ['factory_reset','Factory reset','Erase all Bluetooth bonds, names and settings? This cannot be undone.',cmd.factory_reset]
    ]) this.element(id).onclick = async () => {
      if (await this.confirm(title, message)) await this.operate(title, () => this.command([command]), {refresh: false});
    };
    this.element('rename_device').onclick = () => this.openName('rename_dialog','rename_input',this.state.baseName);
    for (const [button, dialog, input, limit, label] of [
      ['rename_save','rename_dialog','rename_input',PAGER_PROTOCOL.limits.device_name,'Base name'],
      ['slot_name_save','slot_name_dialog','slot_name_input',PAGER_PROTOCOL.limits.slot_name,'Slot name']
    ]) this.element(button).onclick = event => {
      event.preventDefault();
      try {
        const bytes = this.nameBytes(this.element(input).value, limit, label);
        const payload = dialog === 'rename_dialog' ? [cmd.set_device_name,...bytes] : [cmd.set_slot_name,this.renamingSlot,...bytes];
        this.element(dialog).close('default');
        this.operate('Renaming', () => this.command(payload));
      } catch (error) { this.log(error.message,'err'); this.element(input).focus(); }
    };
  }
}
