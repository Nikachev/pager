// Behaviour tests run the shipped JS modules with a deterministic WebUSB/DOM mock.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const tick = () => new Promise(resolve => setImmediate(resolve));
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => {resolve=a;reject=b;}); return {promise,resolve,reject}; };
function runtime() {
  const context = vm.createContext({TextEncoder,TextDecoder,Uint8Array,DataView,setTimeout,clearTimeout,Date});
  vm.runInContext(['protocol_spec.js','protocol_codec.js','usb_session.js','usb_app.js'].map(name => fs.readFileSync(path.join(root,'web',name),'utf8')).join('\n') + '\nglobalThis.api={PagerSession,PagerApp,PagerCodec,PAGER_PROTOCOL};',context);
  return context.api;
}
function stateBytes() {
  // Mac in slot1, empty slots2/3, HID ready.
  const bytes=[5,1,5,0,0,0,1,0,0,1];
  for (const text of ['Mac','','','AA:BB','','','Pager']) {const raw=new TextEncoder().encode(text);bytes.push(raw.length,...raw);}
  return Uint8Array.from(bytes);
}
class Device {
  constructor(api) {
    this.api=api;this.opened=false;this.claims=0;this.releases=0;this.closes=0;
    this.configuration={interfaces:[{interfaceNumber:4,alternate:{interfaceClass:255,endpoints:[{direction:'in',endpointNumber:5},{direction:'out',endpointNumber:5}]}}]};
    this.queue=[];this.readers=[];this.sent=[];this.auto=true;
  }
  async open(){if(this.openError)throw Error('open failure');this.opened=true;}
  async claimInterface(){if(this.claimError)throw Error('claim failure');this.claims++;}
  async releaseInterface(){this.releases++;if(this.releaseError)throw Error('release failure');}
  async close(){this.closes++;this.opened=false;if(!this.keepReader)for(const reader of this.readers.splice(0))reader.reject(Error('closed'));}
  transferIn(){if(this.queue.length)return Promise.resolve(this.queue.shift());const d=deferred();this.readers.push(d);return d.promise;}
  push(bytes){const result={status:'ok',data:new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength)};const reader=this.readers.shift();reader?reader.resolve(result):this.queue.push(result);}
  frame(kind,id,payload){this.push(this.api.PagerCodec.encode(kind,id,Uint8Array.from(payload)));}
  async transferOut(endpoint,bytes){
    const request=this.api.PagerCodec.decode(bytes);this.sent.push(request);
    if(this.writeError)throw Error('write failure');
    if(this.auto)this.frame(2,request.id,request.payload[0]===3?stateBytes():request.payload[0]===1?new TextEncoder().encode('PONG'):[0]);
    return {status:'ok',bytesWritten:this.shortWrite?bytes.length-1:bytes.length};
  }
}
class Element {
  constructor(doc,tag='div'){this.doc=doc;this.tagName=tag;this.children=[];this.listeners=new Map();this.disabled=false;this.open=false;this.value='';this.returnValue='';}
  get childElementCount(){return this.children.length;}
  get firstElementChild(){return this.children[0];}
  append(child){child.parent=this;this.children.push(child);}
  remove(){this.parent.children=this.parent.children.filter(c=>c!==this);}
  replaceChildren(){this.children=[];}
  setAttribute(key,value){this[key]=value;}
  focus(){if(!this.disabled)this.doc.activeElement=this;}
  showModal(){this.open=true;}
  close(value){this.open=false;if(value!==undefined)this.returnValue=value;this.dispatch('close');}
  addEventListener(name,fn,options={}){const values=this.listeners.get(name)||[];values.push({fn,once:options.once});this.listeners.set(name,values);}
  dispatch(name,event={}){for(const item of [...(this.listeners.get(name)||[])]){if(item.once)this.listeners.set(name,this.listeners.get(name).filter(i=>i!==item));item.fn(event);}}
  querySelector(){return this.cancelButton;}
}
class Document {
  constructor(){this.elements=new Map();this.activeElement=null;for(const id of ['log','slots','usb_status','connect','disconnect','device_hint','ble_status','bluetooth_toggle','rename_device','cancel_pairing','get_logs','reboot','factory_reset','text','type','rename_dialog','slot_name_dialog','confirm_dialog','rename_input','slot_name_input','rename_save','slot_name_save','confirm_title','confirm_message','confirm_accept']){const element=new Element(this);element.id=id;this.elements.set(id,element);}
    for(const id of ['rename_dialog','slot_name_dialog','confirm_dialog'])this.elements.get(id).cancelButton=new Element(this,'button');
  }
  getElementById(id){if(this.elements.has(id))return this.elements.get(id);const walk=nodes=>{for(const node of nodes){if(node.id===id)return node;const found=walk(node.children);if(found)return found;}};return walk(this.elements.get('slots').children);}
  createElement(tag){return new Element(this,tag);}
}
async function appFixture(){
  const api=runtime(),device=new Device(api),doc=new Document();
  const listeners={};const usb={requestDevice:async()=>device,addEventListener:(name,fn)=>listeners[name]=fn};
  const app=new api.PagerApp(doc,usb,{timeout:1000});
  await doc.getElementById('connect').onclick();
  return {api,device,doc,app,listeners};
}

test('open/claim failures close partial resources and leave no session',async()=>{
 for(const phase of ['openError','claimError']){const api=runtime(),device=new Device(api),session=new api.PagerSession();device[phase]=true;await assert.rejects(session.connect(device),/failure/);assert.equal(session.connected,false);assert.equal(device.opened,false);assert.equal(device.releases,0);if(phase==='claimError')assert.equal(device.closes,1);}
});
test('split/combined responses and uint32 wrap preserve request correlation',async()=>{
 const api=runtime(),device=new Device(api),session=new api.PagerSession();device.auto=false;await session.connect(device);session.nextId=0xffffffff;
 const first=session.call([1]),second=session.call([1]);await tick();assert.deepEqual(device.sent.map(r=>r.id),[0xffffffff,1]);
 const a=api.PagerCodec.encode(2,1,Uint8Array.from([22])),b=api.PagerCodec.encode(2,0xffffffff,Uint8Array.from([11]));
 const joined=Uint8Array.from([...a,...b]);device.push(joined.slice(0,7));device.push(joined.slice(7));
 assert.deepEqual(Array.from(await first),[11]);assert.deepEqual(Array.from(await second),[22]);await session.disconnect();
});
test('timeout rejects all pending calls, clears timers and releases/closes',async()=>{
 const api=runtime(),device=new Device(api),session=new api.PagerSession({timeout:15});device.auto=false;await session.connect(device);
 const results=await Promise.allSettled([session.call([1]),session.call([3])]);assert.ok(results.every(r=>r.status==='rejected'&&/timeout/.test(r.reason.message)));await tick();
 assert.equal(device.releases,1);assert.equal(device.closes,1);assert.equal(session.connected,false);
});
test('disconnect rejects pending; stale reader cannot update or disconnect new session',async()=>{
 const api=runtime(),old=new Device(api),fresh=new Device(api);old.auto=false;old.keepReader=true;let events=0,failures=0;
 const session=new api.PagerSession({onEvent:()=>events++,onDisconnect:()=>failures++});await session.connect(old);
 const pending=session.call([1]);const rejected=assert.rejects(pending,/disconnected/);await session.disconnect();await rejected;await session.connect(fresh);
 old.frame(3,0,[1,1,0,0,0]);await tick();assert.equal(events,0);assert.equal(failures,0);assert.equal(session.connected,true);assert.equal(new TextDecoder().decode(await session.call([1])),'PONG');await session.disconnect();
});
test('read/write/protocol failures use the same cleanup even when release fails',async()=>{
 for(const mode of ['writeError','shortWrite','invalidKind','invalidEvent']){
  const api=runtime(),device=new Device(api),session=new api.PagerSession();device.releaseError=true;await session.connect(device);device.auto=false;
  if(['writeError','shortWrite'].includes(mode))device[mode]=true;
  const pending=session.call([1]);const rejected=assert.rejects(pending);await tick();
  if(mode==='invalidKind')device.frame(4,device.sent[0].id,[]);
  if(mode==='invalidEvent')device.frame(3,0,[1]);
  await rejected;await tick();assert.equal(session.connected,false);assert.equal(device.releases,1);assert.equal(device.closes,1);
 }
});
test('malformed state disconnects and cleans the actual device',async()=>{
 const {app,device,doc}=await appFixture();device.auto=false;const refreshing=app.refresh();const rejected=assert.rejects(refreshing,/schema/);await tick();device.frame(2,device.sent.at(-1).id,[99]);await rejected;
 assert.equal(app.session.connected,false);assert.equal(device.closes,1);assert.equal(doc.getElementById('type').disabled,true);
});
test('event burst/gap/overflow coalesces one active refresh and one followup',async()=>{
 const {app,device}=await appFixture();device.auto=false;const start=device.sent.length;
 for(let i=1;i<=30;i++)device.frame(3,0,[i===30?255:1,i,0,0,0]);await sleep(35);assert.equal(device.sent.length,start+1);
 for(let i=31;i<=60;i++)device.frame(3,0,[1,i,0,0,0]);await sleep(35);assert.equal(device.sent.length,start+1);
 device.frame(2,device.sent.at(-1).id,stateBytes());await tick();assert.equal(device.sent.length,start+2);
 device.frame(2,device.sent.at(-1).id,stateBytes());await tick();assert.equal(app.refreshing,null);await app.disconnect();
});
test('only one operation; queued refresh follows it; text stays on device error',async()=>{
 const {app,device,doc}=await appFixture();device.auto=false;const operation=app.operate('first',()=>app.session.call([1]));await tick();
 assert.equal(doc.getElementById('type').disabled,true);assert.equal(await app.operate('second',()=>{throw Error('must not run');}),false);
 await app.refresh();device.auto=true;device.frame(2,device.sent.at(-1).id,[0]);await operation;assert.equal(doc.getElementById('type').disabled,false);
 doc.getElementById('text').value='hello';device.auto=false;const sending=doc.getElementById('type').onclick();await tick();device.auto=true;device.frame(5,device.sent.at(-1).id,[6]);await sending;assert.equal(doc.getElementById('text').value,'hello');await app.disconnect();
});
test('UTF-8 names and typing limits reject before transfer without leaking values',async()=>{
 const {app,device,doc}=await appFixture();let sent=device.sent.length;
 assert.throws(()=>app.nameBytes('я'.repeat(13),24,'Base name'),/24/);assert.equal(app.nameBytes('я'.repeat(12),24,'Base name').length,24);
 doc.getElementById('text').value='я'.repeat(129);await doc.getElementById('type').onclick();assert.equal(device.sent.length,sent);
 app.openName('rename_dialog','rename_input','old');doc.getElementById('rename_input').value='я'.repeat(13);doc.getElementById('rename_save').onclick({preventDefault(){}});assert.equal(device.sent.length,sent);assert.equal(doc.getElementById('rename_dialog').open,true);
 assert.ok(!doc.getElementById('log').children.some(r=>r.textContent.includes('яя')));await app.disconnect();
});
test('logs bounded by rows and message size; slot refresh preserves keyboard focus',async()=>{
 const {app,doc}=await appFixture();for(let i=0;i<250;i++)app.log('x'.repeat(5000));assert.equal(doc.getElementById('log').childElementCount,200);assert.ok(doc.getElementById('log').firstElementChild.textContent.length<4200);
 doc.getElementById('slot_0_rename').focus();app.render();assert.equal(doc.activeElement.id,'slot_0_rename');await app.disconnect();
});
test('confirmation starts on Cancel, Escape/cancel prevents command, focus returns',async()=>{
 const {app,doc,device}=await appFixture();const previous=doc.getElementById('factory_reset');previous.focus();let sent=device.sent.length;
 const pending=app.confirm('Reset','Erase');const dialog=doc.getElementById('confirm_dialog');assert.equal(doc.activeElement,dialog.cancelButton);assert.equal(await app.confirm('again','again'),false);
 dialog.close('cancel');assert.equal(await pending,false);assert.equal(doc.activeElement,previous);assert.equal(device.sent.length,sent);
 const accepted=app.confirm('Reset','Erase');dialog.close('default');assert.equal(await accepted,true);await app.disconnect();
});
test('physical disconnect closes dialogs, rejects work and releases resources',async()=>{
 const {app,doc,device,listeners}=await appFixture();const answer=app.confirm('Reset','Erase');listeners.disconnect({device});assert.equal(await answer,false);await tick();assert.equal(app.session.connected,false);assert.equal(device.closes,1);assert.equal(doc.getElementById('connect').hidden,false);
});
test('cancel during an asynchronous open closes the eventual device',async()=>{
 const api=runtime(),device=new Device(api),session=new api.PagerSession();const opened=deferred();
 device.open=async()=>{await opened.promise;device.opened=true;};
 const connecting=session.connect(device);const rejected=assert.rejects(connecting,/disconnected/);await tick();await session.disconnect();opened.resolve();await rejected;
 assert.equal(device.opened,false);assert.equal(device.closes,1);assert.equal(device.claims,0);
});
test('wrap skips an outstanding ID; malformed error fails all pending',async()=>{
 const api=runtime(),device=new Device(api),session=new api.PagerSession();device.auto=false;await session.connect(device);
 const first=session.call([1]);session.nextId=1;const second=session.call([1]);const results=Promise.allSettled([first,second]);await tick();
 assert.deepEqual(device.sent.map(r=>r.id),[1,2]);device.frame(5,1,[]);assert.ok((await results).every(r=>r.status==='rejected'));await tick();assert.equal(device.closes,1);
});
test('initial connect state failure is visible and closes USB; reconnect succeeds',async()=>{
 const api=runtime(),device=new Device(api),doc=new Document();let bad=true;
 device.transferOut=async(endpoint,bytes)=>{const request=api.PagerCodec.decode(bytes);device.sent.push(request);device.frame(2,request.id,bad?[99]:stateBytes());return {status:'ok',bytesWritten:bytes.length};};
 const app=new api.PagerApp(doc,{requestDevice:async()=>device,addEventListener(){}});
 await doc.getElementById('connect').onclick();assert.equal(app.session.connected,false);assert.ok(doc.getElementById('log').children.some(row=>/schema/.test(row.textContent)));
 bad=false;await doc.getElementById('connect').onclick();assert.equal(app.session.connected,true);assert.ok(app.state.hidReady);await app.disconnect();
});
test('strict command ACK rejects a response with trailing bytes',async()=>{
 const {app,device}=await appFixture();device.auto=false;const command=app.command([6,0]);const rejected=assert.rejects(command,/acknowledgement/);await tick();device.frame(2,device.sent.at(-1).id,[0,0]);await rejected;await app.disconnect();
});
test('unsupported browser disables connection and explains requirement',()=>{
 const api=runtime(),doc=new Document();new api.PagerApp(doc,undefined);assert.equal(doc.getElementById('connect').disabled,true);assert.match(doc.getElementById('device_hint').textContent,/Chrome/);
});
