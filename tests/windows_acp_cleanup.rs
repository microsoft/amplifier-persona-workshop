//! Real Windows kernel cleanup on both successful and failed ACP negotiation.
#![cfg(windows)]
use persona_workshop::{config::Extractor, worker};
use serde_json::Value;
use std::collections::BTreeMap;
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    },
};

#[tokio::test]
async fn closing_successful_or_failed_acp_client_stops_spawned_local_descendants() {
    for failure in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let output = home.path().join("fixture-pids.json");
        let script = r#"
import json, os, subprocess, sys
for line in sys.stdin:
    r=json.loads(line)
    if r.get('method') == 'initialize':
        child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        with open(sys.argv[1],'w') as out: json.dump([os.getpid(),child.pid],out)
        response={'jsonrpc':'2.0','id':r['id']}
        response.update({'error':{'code':-32603,'message':'fixture rejection'}} if sys.argv[2]=='fail' else {'result':{'protocolVersion':1,'agentCapabilities':{}}})
        print(json.dumps(response),flush=True)
    elif r.get('method') == 'session/new':print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{'sessionId':'fixture'}}),flush=True)
    elif r.get('method') == 'session/prompt':
        print(json.dumps({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'Fixture complete'}}}}),flush=True)
        print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{'stopReason':'end_turn'}}),flush=True)
"#;
        let config = Extractor {
            command: "python".into(),
            args: vec![
                "-c".into(),
                script.into(),
                output.to_string_lossy().into_owned(),
                if failure { "fail" } else { "pass" }.into(),
            ],
            cwd: home.path().to_string_lossy().into_owned(),
            env: BTreeMap::new(),
            timeout_seconds: 10,
        };
        let result = worker::run(&config, "A disposable protocol fixture").await;
        let pids: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let mut alive = vec![];
        for pid in pids.as_array().unwrap() {
            let pid = pid.as_u64().unwrap() as u32;
            // SAFETY: test-owned process IDs; valid handles are closed exactly once.
            unsafe {
                let process = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
                if !process.is_null() {
                    if WaitForSingleObject(process, 3000) != WAIT_OBJECT_0 {
                        alive.push(pid);
                        TerminateProcess(process, 1); // failed regression must not leak its fixture
                        WaitForSingleObject(process, 3000);
                    }
                    CloseHandle(process);
                } else {
                    assert_eq!(
                        std::io::Error::last_os_error().raw_os_error(),
                        Some(87),
                        "Cannot inspect fixture process"
                    );
                }
            }
        }
        assert!(
            alive.is_empty(),
            "local ACP descendants survived client drop: {alive:?}"
        );
        assert_eq!(result.is_err(), failure);
    }
}
