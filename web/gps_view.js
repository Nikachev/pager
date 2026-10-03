// GPS data stays in the page. Time zones use the bundled geographic lookup,
// then the browser's IANA rules for the board's UTC instant (including DST).
const PagerGps = (() => {
  function decode(bytes) {
    const text = new TextDecoder('utf-8', {fatal: true}).decode(bytes);
    const fields = {};
    for (const entry of text.split(';')) {
      const i = entry.indexOf('=');
      if (i < 1 || Object.hasOwn(fields, entry.slice(0, i))) throw Error('Invalid GPS response');
      fields[entry.slice(0, i)] = entry.slice(i + 1);
    }
    if (!['0','1'].includes(fields.supported)) throw Error('Invalid GPS support flag');
    if (fields.supported === '0') return {supported: false};
    const flag = key => {
      if (!['0','1'].includes(fields[key])) throw Error(`Invalid GPS ${key}`);
      return fields[key] === '1';
    };
    const number = key => {
      if (!/^\d+$/.test(fields[key] || '')) throw Error(`Invalid GPS ${key}`);
      return Number(fields[key]);
    };
    const connected = flag('connected'), fix = flag('fix');
    let position = null;
    if (fix) {
      const lat = Number(fields.latitude_e6) / 1e6, lon = Number(fields.longitude_e6) / 1e6;
      if (!fields.latitude_e6 || !fields.longitude_e6 || !Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180) throw Error('Invalid GPS coordinates');
      position = {lat, lon};
    }
    let lastPosition = null;
    if (fields.last_latitude_e6 !== undefined) {
      const lat = Number(fields.last_latitude_e6) / 1e6, lon = Number(fields.last_longitude_e6) / 1e6;
      if (!Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180) throw Error('Invalid last GPS coordinates');
      lastPosition = {lat, lon, age: number('last_fix_age_ms'), utcMs: number('last_fix_utc_ms') || null};
    }
    const source = fields.time_source;
    // Older firmware exposes diagnostics without a disciplined UTC clock.
    if (source === undefined) return {supported: true, connected, position, utcMs: null, source: 'unsynced', age: null, satellites: number('satellites')};
    if (!['unsynced','gps','holdover'].includes(source)) throw Error('Invalid GPS clock source');
    const utcMs = source === 'unsynced' ? null : number('utc_ms');
    if (utcMs !== null && (!Number.isSafeInteger(utcMs) || utcMs > 8640000000000000)) throw Error('Invalid board UTC');
    return {supported: true, connected, position, lastPosition, utcMs, source, age: source === 'unsynced' ? null : number('time_age_ms'), satellites: number('satellites')};
  }
  function time(utcMs, zone) {
    return new Intl.DateTimeFormat('ru-RU', {timeZone: zone, year:'numeric', month:'2-digit', day:'2-digit', hour:'2-digit', minute:'2-digit', second:'2-digit', hourCycle:'h23'}).format(new Date(utcMs));
  }
  function zone(position) {
    const name = tzlookup(position.lat, position.lon);
    // Unsupported IANA names are handled by the caller, with an explicit UTC fallback.
    time(0, name);
    return name;
  }
  return {decode, time, zone};
})();
