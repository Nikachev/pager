const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const context = vm.createContext({TextEncoder, TextDecoder, Uint8Array, DataView});
vm.runInContext(fs.readFileSync(path.join(root, 'web/protocol_spec.js'), 'utf8') +
  fs.readFileSync(path.join(root, 'web/protocol_codec.js'), 'utf8') + '\nglobalThis.codec = PagerCodec;', context);
const vectors = JSON.parse(fs.readFileSync(path.join(__dirname, 'fixtures/protocol_vectors.json')));
for (const v of vectors) {
  const payload = Uint8Array.from(Buffer.from(v.payload_hex, 'hex'));
  const expected = Uint8Array.from(Buffer.from(v.frame_hex, 'hex'));
  assert.equal(Buffer.from(context.codec.encode(v.kind, v.request_id, payload)).toString('hex'), v.frame_hex);
  const decoded = context.codec.decode(expected);
  assert.equal(decoded.id, v.request_id);
  assert.equal(Buffer.from(decoded.payload).toString('hex'), v.payload_hex);
  for (let split = 0; split < expected.length; split++) assert.throws(() => context.codec.decode(expected.slice(0, split)));
  const bad = expected.slice(); bad[5] = 255;
  assert.throws(() => context.codec.decode(bad), /kind/);
}
const state = Uint8Array.from([5,1,1,255,255,0,0,0,0,0,0,0,0,0,0,0,0]);
assert.equal(context.codec.decodeState(state).baseName, '');
for (let size = 0; size < state.length; size++) assert.throws(() => context.codec.decodeState(state.slice(0,size)));
assert.throws(() => context.codec.decodeState(Uint8Array.from([...state,0])), /Trailing/);
console.log('JS protocol golden vectors and strict state boundaries passed');

const page = fs.readFileSync(path.join(root, 'webusb_client.html'), 'utf8');
new vm.Script(page.split('<script>')[1].split('</script>')[0]);
