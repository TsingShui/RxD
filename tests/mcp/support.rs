use serde_json::{Value, json};
use std::collections::VecDeque;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

/// One real-process wire driver shared by every MCP E2E scenario.
pub struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<String>,
    next_id: u64,
    backlog: VecDeque<Value>,
}
impl Client {
    pub fn start(extra: &[&str]) -> Self {
        Self::start_with_env(extra, &[])
    }
    pub fn start_with_env(extra: &[&str], env: &[(&str, &str)]) -> Self {
        Self::start_binary_with_env(
            std::path::Path::new(env!("CARGO_BIN_EXE_rxd")),
            extra,
            env,
            &[],
        )
    }
    pub fn start_binary_with_env(
        binary: &std::path::Path,
        extra: &[&str],
        env: &[(&str, &str)],
        remove: &[&str],
    ) -> Self {
        let mut command = Command::new(binary);
        for key in remove {
            command.env_remove(key);
        }
        command.args(["mcp"]).args(extra).envs(env.iter().copied());
        Self::start_command(command)
    }
    pub fn start_command(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start actual rxd MCP server");
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match stdout.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            input: child.stdin.take(),
            output,
            child,
            next_id: 0,
            backlog: VecDeque::new(),
        }
    }
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        if let Some(index) = self
            .backlog
            .iter()
            .position(|response| response["id"] == id)
        {
            return self.backlog.remove(index).unwrap();
        }
        loop {
            let response = self.read_wire();
            if response["id"] == id {
                return response;
            }
            self.backlog.push_back(response);
        }
    }
    #[cfg_attr(not(feature = "bundled-kuna"), allow(dead_code))]
    pub fn receive(&mut self) -> Value {
        self.backlog.pop_front().unwrap_or_else(|| self.read_wire())
    }
    fn read_wire(&self) -> Value {
        let line = self
            .output
            .recv_timeout(Duration::from_secs(15))
            .expect("actual MCP server response within E2E timeout");
        serde_json::from_str(&line).expect("stdout contains only MCP JSON-RPC frames")
    }
    pub fn send(&mut self, value: Value) {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            self.next_id = self.next_id.max(id);
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        self.send_raw(&bytes);
        self.send_raw(b"\n");
    }
    pub fn send_raw(&mut self, bytes: &[u8]) {
        let input = self.input.as_mut().unwrap();
        input.write_all(bytes).unwrap();
        input.flush().unwrap();
    }
    pub fn initialize(&mut self) {
        let response=self.request("initialize",json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rxd-e2e","version":"1"}}));
        assert!(response.get("error").is_none(), "{response}");
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }
    pub fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }
    pub fn disconnect(&mut self) {
        self.input.take();
    }
    #[cfg(feature = "bundled-kuna")]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn wait_exit(&mut self) -> std::process::ExitStatus {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "MCP server did not shut down on EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Analysis workers are children of the service process; find one for RSS checks.
#[cfg(feature = "bundled-kuna")]
pub(super) fn child_worker(parent: u32) -> u32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let output = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid="])
            .output()
            .unwrap();
        let child = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse::<u32>().ok()?;
                let ppid = fields.next()?.parse::<u32>().ok()?;
                (ppid == parent).then_some(pid)
            });
        if let Some(pid) = child {
            return pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "analysis worker did not appear"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
