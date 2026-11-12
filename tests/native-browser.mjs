import fs from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'../ui/tests/node_modules/playwright/index.mjs');
const input=JSON.parse(await fs.readFile(process.argv[2],'utf8'));
const browser=await chromium.launch({headless:true,...(process.env.CHROMIUM_EXECUTABLE?{executablePath:process.env.CHROMIUM_EXECUTABLE}:{}),args:['--no-sandbox','--disable-dev-shm-usage']});
try{
 const page=await browser.newPage({viewport:{width:1100,height:800}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(input.base+'/#setup='+input.cap);await page.getByRole('heading',{name:'Set up Persona Workshop'}).waitFor();assert(!page.url().includes('setup='));
 if(process.env.UI_ARTIFACTS){await fs.mkdir(process.env.UI_ARTIFACTS,{recursive:true});await page.screenshot({path:path.join(process.env.UI_ARTIFACTS,'native-setup-desktop.png')});}
 // The account commit succeeds, but its response is lost. The UI must inspect
 // existing state, never create a replacement account or ask for new setup.
 await page.route('**/auth/first-account',async route=>{const response=await route.fetch();assert.equal(response.status(),200);await route.abort('connectionfailed');});
 await page.locator('#field-password').fill(input.password);await page.getByRole('button',{name:'Create account',exact:true}).click();
 await page.waitForFunction(()=>window.personaWorkshop?.snapshot().view || [...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='Sign in'));
 assert.equal(await page.getByRole('heading',{name:'Set up Persona Workshop'}).count(),0);
 if(!await page.evaluate(()=>!!window.personaWorkshop?.snapshot().view)){await page.locator('#field-password').fill(input.password);await page.getByRole('button',{name:'Sign in',exact:true}).click();await page.waitForFunction(()=>window.personaWorkshop?.snapshot().view);}
 assert.equal(await page.evaluate(()=>sessionStorage.getItem('persona-workshop-first-account')),null);
 await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
 if(process.env.UI_ARTIFACTS)await page.screenshot({path:path.join(process.env.UI_ARTIFACTS,'native-empty-mobile.png')});
 // Update display and recovery are private UI state, never agent view commands.
 await page.getByRole('button',{name:'App updates',exact:true}).click();
 await page.getByRole('heading',{name:'App updates',exact:true}).waitFor();
 assert.equal(await page.getByRole('button',{name:'Check for an update',exact:true}).count(),1);
 await page.keyboard.press('Escape');assert.equal(await page.getByRole('heading',{name:'App updates',exact:true}).count(),0);
 let dispatched=0, originalRequest;
 await page.route('**/api/v1/desktop/updates',async route=>{
   if(route.request().method()==='POST'){
     dispatched++;originalRequest=route.request().postDataJSON();
     // The action is not sent to the real updater in this browser fixture.
     await route.abort('connectionfailed');return;
   }
   await route.fulfill({contentType:'application/json',body:JSON.stringify({supported:true,current:originalRequest?{id:'fixture-only',state:'ready',version:'0.7.0',request_id:originalRequest.request_id}:null})});
 });
 await page.getByRole('button',{name:'App updates',exact:true}).click();
 await page.getByRole('button',{name:'Check for an update',exact:true}).click();
 await page.getByText('The request was not confirmed.',{exact:false}).waitFor();
 await page.getByRole('button',{name:'Refresh status',exact:true}).click();
 await page.getByText('Ready to install',{exact:false}).waitFor();
 assert.equal(dispatched,1);assert.equal(await page.evaluate(()=>sessionStorage.getItem('persona-native-update-intent')),null);
 assert.equal(await page.getByRole('button',{name:'Retry the same request',exact:true}).count(),0);
 await page.keyboard.press('Escape');
 await page.getByRole('button',{name:/Sign out/}).first().click();await page.getByRole('button',{name:'Sign in',exact:true}).waitFor();await page.getByText('Use the password you chose for this workshop.').waitFor();assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
 if(process.env.UI_ARTIFACTS)await page.screenshot({path:path.join(process.env.UI_ARTIFACTS,'native-login-mobile.png')});
 assert.deepEqual(errors,[]);console.log('Native browser: first account, fragment removal, lost committed response, ordinary login, empty workspace, mobile fit and no script errors passed');
}finally{await browser.close();}
