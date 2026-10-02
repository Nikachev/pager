// Each connection owns its reader, write queue, pending calls and USB resources.
class PagerSession {
  constructor({onEvent = () => {}, onDisconnect = () => {}, timeout = 12000} = {}) {
    this.onEvent = onEvent;
    this.onDisconnect = onDisconnect;
    this.timeout = timeout;
    this.session = null;
    this.generation = 0;
    this.nextId = 1;
  }

  get connected() { return !!this.session?.active; }

  async connect(device) {
    await this.disconnect();
    const session = {
      device, token: ++this.generation, active: true, claimed: false,
      pending: new Map(), rx: new Uint8Array(), writes: Promise.resolve()
    };
    this.session = session;
    try {
      await device.open();
      this.requireCurrent(session);
      if (!device.configuration) await device.selectConfiguration(1);
      this.requireCurrent(session);
      for (const item of device.configuration.interfaces) {
        const alt = item.alternate;
        if (alt.interfaceClass !== 255) continue;
        const input = alt.endpoints.find(e => e.direction === 'in');
        const output = alt.endpoints.find(e => e.direction === 'out');
        if (input && output) {
          session.iface = item.interfaceNumber;
          session.input = input.endpointNumber;
          session.output = output.endpointNumber;
          break;
        }
      }
      if (session.iface === undefined) throw Error('Pager control interface not found');
      await device.claimInterface(session.iface);
      session.claimed = true;
      this.requireCurrent(session);
      this.read(session);
    } catch (error) {
      await this.cleanup(session, error);
      throw error;
    }
  }

  isCurrent(session) {
    return session.active && this.session === session && session.token === this.generation;
  }

  requireCurrent(session) {
    if (!this.isCurrent(session)) throw Error('USB disconnected');
  }

  async cleanup(session, error = Error('USB disconnected')) {
    session.active = false;
    if (this.session === session) this.session = null;
    for (const pending of session.pending.values()) pending.reject(error);
    session.pending.clear();
    session.rx = new Uint8Array();
    // A connect cancellation may reach cleanup again after open/claim completes.
    if (session.claimed) {
      session.claimed = false;
      try { await session.device.releaseInterface(session.iface); } catch (_) {}
    }
    if (session.device.opened) {
      try { await session.device.close(); } catch (_) {}
    }
  }

  async disconnect() {
    ++this.generation;
    const session = this.session;
    if (session) await this.cleanup(session);
  }

  async fail(session, error) {
    if (!this.isCurrent(session)) return;
    // Notify before awaits so a new connection cannot receive the old failure.
    const cleanup = this.cleanup(session, error);
    this.onDisconnect(error);
    await cleanup;
  }

  call(payload) {
    const session = this.session;
    if (!session || !this.isCurrent(session)) return Promise.reject(Error('USB disconnected'));
    let id = this.nextId >>> 0 || 1;
    while (session.pending.has(id)) id = (id + 1) >>> 0 || 1;
    this.nextId = (id + 1) >>> 0 || 1;
    let frame;
    try { frame = PagerCodec.encode(PAGER_PROTOCOL.frame_kinds.command, id, Uint8Array.from(payload)); }
    catch (error) { return Promise.reject(error); }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!session.pending.has(id)) return;
        this.fail(session, Error('Device response timeout'));
      }, this.timeout);
      session.pending.set(id, {
        resolve: value => { clearTimeout(timer); resolve(value); },
        reject: error => { clearTimeout(timer); reject(error); }
      });
      const send = async () => {
        this.requireCurrent(session);
        const result = await session.device.transferOut(session.output, frame);
        this.requireCurrent(session);
        if (result.status !== 'ok' || result.bytesWritten !== frame.length) {
          throw Error('USB write failed');
        }
      };
      session.writes = session.writes.then(send).catch(error => this.fail(session, error));
    });
  }

  async receive(session) {
    const header = PAGER_PROTOCOL.header_size;
    while (true) {
      this.requireCurrent(session);
      if (session.rx.length >= header) {
        const view = new DataView(session.rx.buffer, session.rx.byteOffset, session.rx.byteLength);
        const length = view.getUint16(10, true);
        if (length > PAGER_PROTOCOL.max_payload) throw Error('Invalid response length');
        const total = header + length;
        if (session.rx.length >= total) {
          const raw = session.rx.slice(0, total);
          session.rx = session.rx.slice(total);
          return PagerCodec.decode(raw);
        }
      }
      const result = await session.device.transferIn(session.input, 64);
      this.requireCurrent(session);
      if (result.status !== 'ok' || !result.data) throw Error('USB read failed');
      const incoming = new Uint8Array(result.data.buffer, result.data.byteOffset, result.data.byteLength);
      const joined = new Uint8Array(session.rx.length + incoming.length);
      joined.set(session.rx); joined.set(incoming, session.rx.length);
      session.rx = joined;
      if (joined.length > header + PAGER_PROTOCOL.max_payload + 64) throw Error('USB receive buffer overflow');
    }
  }

  async read(session) {
    try {
      while (this.isCurrent(session)) {
        const frame = await this.receive(session);
        this.requireCurrent(session);
        const kinds = PAGER_PROTOCOL.frame_kinds;
        if (frame.kind === kinds.event) {
          if (frame.id !== 0 || frame.payload.length !== 5 || ![1,255].includes(frame.payload[0])) {
            throw Error('Invalid device event');
          }
          this.onEvent(frame.payload);
          continue;
        }
        if (![kinds.response, kinds.error].includes(frame.kind) || frame.id === 0) {
          throw Error('Unexpected response kind');
        }
        if (frame.kind === kinds.error && frame.payload.length !== 1) throw Error('Invalid device error');
        const pending = session.pending.get(frame.id);
        if (!pending) continue;
        session.pending.delete(frame.id);
        if (frame.kind === kinds.error) pending.reject(Error(PagerErrors[frame.payload[0]] || 'Unknown device error'));
        else pending.resolve(frame.payload);
      }
    } catch (error) { await this.fail(session, error); }
  }
}

const PagerErrors = {
  1: 'Bad request', 2: 'Unsupported command', 3: 'Device busy or timed out',
  4: 'Firmware update failed', 5: 'Keyboard is not ready or subscribed',
  6: 'Text contains an unsupported character', 7: 'Bluetooth connection was lost',
  8: 'Command queue is full'
};
