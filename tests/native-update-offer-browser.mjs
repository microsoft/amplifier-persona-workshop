// Render the exact offer assets with a controlled status transport. This checks
// browser request/copy behavior only; native update effects have separate tests.
import fs from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';
import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'../ui/tests/node_modules/playwright/index.mjs');
const source=await fs.readFile(new URL('../src/native_update_offer.rs',import.meta.url),'utf8');
const asset=name=>{const m=source.match(new RegExp('const '+name+': &str = r#"([\\s\\S]*?)"#;'));assert(m,name);return m[1];};
let status={state:'ready',version:'0.7.2',id:'fixture-only',up_to_date:false},requests=[],lose=true;
const server=http.createServer(async(req,res)=>{
 const staticAsset={'/':['text/html',asset('HTML')],'/offer.css':['text/css',asset('CSS')],'/offer.js':['text/javascript',asset('JS')]}[req.url];
 res.setHeader('Cache-Control','no-store');
 res.setHeader('Content-Security-Policy',"default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'");
 if(staticAsset){res.setHeader('Content-Type',staticAsset[0]);res.end(staticAsset[1]);return;}
 assert.equal(req.headers.authorization,'Bearer fixture-capability');
 res.setHeader('Content-Type','application/json');
 if(req.method==='GET'&&req.url==='/status'){res.end(JSON.stringify(status));return;}
 let raw='';for await(const chunk of req)raw+=chunk;
 const body=JSON.parse(raw);requests.push({path:req.url,body});
 if(req.url==='/close'){res.end('{"closed":true}');return;}
 assert.match(body.request_id,/^[0-9a-f-]{36}$/);assert.deepEqual(Object.keys(body),['request_id']);
 if(lose){lose=false;res.end(JSON.stringify(status));return;}
 status={...status,state:req.url==='/recover'?'rolling_back':'queued',recovery_available:false};res.end(JSON.stringify(status));
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const origin='http://127.0.0.1:'+server.address().port;
const browser=await chromium.launch({headless:true,...(process.env.CHROMIUM_EXECUTABLE?{executablePath:process.env.CHROMIUM_EXECUTABLE}:{}),args:['--no-sandbox','--disable-dev-shm-usage']});
let page,errors=[];
try{
 page=await browser.newPage({viewport:{width:1100,height:800}});page.on('pageerror',e=>errors.push(e.message));
 await page.goto(origin+'/#setup=fixture-capability');await page.getByRole('heading',{name:'Ready to update',exact:true}).waitFor();
 assert(!page.url().includes('setup='));assert.equal(await page.evaluate(()=>sessionStorage.getItem('persona-update-offer')),'fixture-capability');
 if(process.env.UI_ARTIFACTS){await fs.mkdir(process.env.UI_ARTIFACTS,{recursive:true});await page.screenshot({path:path.join(process.env.UI_ARTIFACTS,'update-offer-desktop.png')});}
 await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
 if(process.env.UI_ARTIFACTS)await page.screenshot({path:path.join(process.env.UI_ARTIFACTS,'update-offer-mobile.png')});
 let intercept=true;await page.route('**/apply',async route=>{if(intercept){intercept=false;const response=await route.fetch();assert.equal(response.status(),200);await route.abort('connectionfailed');}else await route.continue();});
 await page.getByRole('button',{name:'Update Persona Workshop',exact:true}).click();await page.getByText(/result may still be pending|previous request was not confirmed/).waitFor();
 await page.waitForTimeout(3200);assert.equal(requests.length,1,'Status polling must not repeat an uncertain request');
 await page.reload();await page.getByRole('heading',{name:'Ready to update',exact:true}).waitFor();
 await page.getByRole('button',{name:'Retry the same update request',exact:true}).click();await page.getByRole('heading',{name:'Update requested',exact:true}).waitFor();
 assert.equal(requests.length,2);assert.deepEqual(requests[0],requests[1],'Explicit retry retains exact request identity across refresh');
 status={...status,state:'recovery_required',recovery_available:true};await page.getByRole('button',{name:'Recover interrupted update',exact:true}).waitFor();
 await page.getByRole('button',{name:'Recover interrupted update',exact:true}).click();await page.getByRole('heading',{name:'Restoring the previous version',exact:true}).waitFor();
 assert.equal(requests[2].path,'/recover');assert.notEqual(requests[2].body.request_id,requests[1].body.request_id);
 status={...status,state:'rolled_back',url:origin+'/fixture-workshop',recovery_available:false};await page.getByRole('link',{name:'Open Persona Workshop',exact:true}).waitFor();
 assert.equal(await page.getByRole('link',{name:'Open Persona Workshop',exact:true}).getAttribute('href'),origin+'/fixture-workshop');
 await page.getByRole('button',{name:'Close this window',exact:true}).click();await page.getByRole('heading',{name:'You can close this tab',exact:true}).waitFor();
 assert.equal(requests.length,4);assert.deepEqual(errors,[]);
 console.log(JSON.stringify({checks:9,scope:'Source-rendered browser assets with controlled transport; no native update effects',passed:true}));
}catch(e){console.error(JSON.stringify({requests,errors,body:page?await page.locator('body').innerText():null}));throw e;}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
