//! A text-only ACP client. Analysis never receives client tool capabilities.
use crate::config::Extractor;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Peer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    pid: u32,
    next: u64,
}
impl Drop for Peer {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.pid > 0 {
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}
impl Peer {
    async fn start(c: &Extractor) -> Result<Self> {
        let mut cmd = Command::new(&c.command);
        cmd.args(&c.args)
            .current_dir(&c.cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for k in [
            "PATH",
            "HOME",
            "USER",
            "LANG",
            "TMPDIR",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "SYSTEMROOT",
            "WINDIR",
        ] {
            if let Ok(v) = std::env::var(k) {
                cmd.env(k, v);
            }
        }
        cmd.envs(&c.env);
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd
            .spawn()
            .context("Could not start the configured ACP extractor")?;
        let pid = child.id().unwrap_or(0);
        let input = child.stdin.take().context("Missing ACP input")?;
        let output = BufReader::new(child.stdout.take().context("Missing ACP output")?);
        Ok(Self {
            child,
            input,
            output,
            pid,
            next: 0,
        })
    }
    async fn write(&mut self, v: Value) -> Result<()> {
        self.input.write_all(format!("{v}\n").as_bytes()).await?;
        self.input.flush().await?;
        Ok(())
    }
    async fn read(&mut self) -> Result<Value> {
        let mut buf = Vec::new();
        loop {
            let b = self.output.fill_buf().await?;
            ensure!(!b.is_empty(), "ACP endpoint closed its output");
            let n = b
                .iter()
                .position(|x| *x == b'\n')
                .map(|p| p + 1)
                .unwrap_or(b.len());
            ensure!(
                buf.len() + n <= 4 * 1024 * 1024,
                "ACP frame exceeds the limit"
            );
            buf.extend_from_slice(&b[..n]);
            self.output.consume(n);
            if buf.last() == Some(&b'\n') {
                break;
            }
        }
        let v: Value = serde_json::from_slice(&buf)?;
        ensure!(v["jsonrpc"] == "2.0", "Invalid ACP response");
        Ok(v)
    }
    async fn reject(&mut self, v: &Value) -> Result<()> {
        if v["method"] == "session/request_permission" {
            self.write(
                json!({"jsonrpc":"2.0","id":v["id"],"result":{"outcome":{"outcome":"cancelled"}}}),
            )
            .await
        } else {
            self.write(json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"This analysis client offers no tool capabilities"}})).await
        }
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        loop {
            let v = self.read().await?;
            if v.get("method").is_none() && v["id"] == id {
                ensure!(v.get("error").is_none(), "ACP request failed");
                return Ok(v["result"].clone());
            }
            if v.get("id").is_some() && v.get("method").is_some() {
                self.reject(&v).await?;
            }
        }
    }
}
pub async fn run(c: &Extractor, prompt: &str) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(c.timeout_seconds), run_inner(c, prompt))
        .await
        .context("Analysis timed out")?
}
async fn run_inner(c: &Extractor, prompt: &str) -> Result<String> {
    let mut peer = Peer::start(c).await?;
    let init=peer.request("initialize",json!({"protocolVersion":1,"clientInfo":{"name":"persona-workshop","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}})).await?;
    ensure!(init["protocolVersion"] == 1, "Unsupported ACP version");
    let session = peer
        .request("session/new", json!({"cwd":c.cwd,"mcpServers":[]}))
        .await?;
    let sid = session["sessionId"]
        .as_str()
        .context("Missing ACP session ID")?
        .to_owned();
    peer.next += 1;
    let request_id = peer.next;
    peer.write(json!({"jsonrpc":"2.0","id":request_id,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":prompt}]}})).await?;
    let mut output = String::new();
    let mut message_id = None;
    loop {
        let v = peer.read().await?;
        if v.get("method").is_none() && v["id"] == request_id {
            ensure!(
                v.get("error").is_none() && v["result"]["stopReason"] == "end_turn",
                "Analysis did not complete"
            );
            ensure!(!output.trim().is_empty(), "Analysis returned no text");
            return Ok(output);
        }
        if v.get("id").is_some() && v.get("method").is_some() {
            peer.reject(&v).await?;
            continue;
        }
        if v["method"] == "session/update" && v["params"]["sessionId"] == sid {
            let u = &v["params"]["update"];
            if matches!(
                u["sessionUpdate"].as_str(),
                Some("tool_call" | "tool_call_update")
            ) {
                let _=peer.write(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}})).await;
                bail!(
                    "The analysis endpoint tried to use tools; configure a text-only analysis endpoint"
                );
            }
            if u["sessionUpdate"] == "agent_message_chunk" && u["content"]["type"] == "text" {
                if let Some(id) = u["messageId"].as_str()
                    && message_id.as_deref() != Some(id)
                {
                    output.clear();
                    message_id = Some(id.to_owned());
                }
                if let Some(text) = u["content"]["text"].as_str() {
                    ensure!(
                        output.len() + text.len() <= 300_000,
                        "Analysis output exceeds the limit"
                    );
                    output.push_str(text);
                }
            }
        }
    }
}
