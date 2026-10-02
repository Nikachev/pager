// Pure wire codec, usable in an autonomous page or a Node VM test.
const PagerCodec = (() => {
  const spec = PAGER_PROTOCOL;
  const magic = new TextEncoder().encode(spec.frame_magic);
  function crc32(bytes) {
    let crc = 0xffffffff;
    for (const byte of bytes) {
      crc ^= byte;
      for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
    }
    return (crc ^ 0xffffffff) >>> 0;
  }
  function encode(kind, id, payload) {
    if (!Object.values(spec.frame_kinds).includes(kind)) throw Error('Invalid frame kind');
    if (!Number.isInteger(id) || id < 0 || id > 0xffffffff) throw Error('Invalid request ID');
    if (payload.length > spec.max_payload) throw Error('Payload exceeds protocol limit');
    const frame = new Uint8Array(spec.header_size + payload.length);
    frame.set(magic); frame[4] = spec.frame_version; frame[5] = kind;
    const view = new DataView(frame.buffer);
    view.setUint32(6, id, true); view.setUint16(10, payload.length, true);
    view.setUint32(12, crc32(payload), true); frame.set(payload, spec.header_size);
    return frame;
  }
  function decode(frame) {
    if (frame.length < spec.header_size) throw Error('Truncated frame');
    if (!magic.every((byte, i) => frame[i] === byte)) throw Error('Invalid frame magic');
    if (frame[4] !== spec.frame_version) throw Error('Incompatible Pager protocol');
    if (!Object.values(spec.frame_kinds).includes(frame[5])) throw Error('Invalid frame kind');
    const view = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
    const length = view.getUint16(10, true);
    if (length > spec.max_payload || frame.length !== spec.header_size + length) throw Error('Invalid frame length');
    const payload = frame.slice(spec.header_size);
    if (crc32(payload) !== view.getUint32(12, true)) throw Error('Invalid frame CRC');
    return {kind: frame[5], id: view.getUint32(6, true), payload};
  }
  function decodeState(bytes) {
    if (bytes.length < 10 || bytes[0] !== spec.state_schema) throw Error('Incompatible state schema');
    if ([1,5,6,7,8,9].some(i => bytes[i] > 1) || bytes[2] > 6 ||
        [3,4].some(i => bytes[i] !== 255 && bytes[i] >= spec.limits.slots)) throw Error('Invalid state fields');
    let offset = 10;
    const decoder = new TextDecoder('utf-8', {fatal: true});
    function string(limit) {
      if (offset >= bytes.length) throw Error('Truncated state');
      const length = bytes[offset++];
      if (length > limit || offset + length > bytes.length) throw Error('Invalid state string');
      const value = decoder.decode(bytes.slice(offset, offset + length)); offset += length;
      return value;
    }
    const names = Array.from({length: spec.limits.slots}, () => string(spec.limits.slot_name));
    const addresses = Array.from({length: spec.limits.slots}, () => string(32));
    const baseName = string(spec.limits.device_name);
    if (offset !== bytes.length) throw Error('Trailing state bytes');
    return {enabled: !!bytes[1], link: bytes[2], active: bytes[3] === 255 ? null : bytes[3],
      connected: bytes[4] === 255 ? null : bytes[4], pairing: !!bytes[5],
      bonds: Array.from(bytes.slice(6,9), Boolean), hidReady: !!bytes[9], names, addresses, baseName};
  }
  return {encode, decode, decodeState, crc32};
})();
