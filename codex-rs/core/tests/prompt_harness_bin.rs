use std::error::Error;
use std::fs;

use assert_cmd::Command;
use core_test_support::load_sse_fixture_with_id;
use tempfile::TempDir;

#[test]
fn prompt_harness_streams_session_event() -> Result<(), Box<dyn Error>> {
    let workspace = TempDir::new()?;
    let codex_home = workspace.path().join("codex_home");
    fs::create_dir(&codex_home)?;

    let prompt_path = workspace.path().join("override.md");
    fs::write(&prompt_path, "system override contents")?;

    // Feed the harness from a local SSE fixture so the test never hits the network.
    let sse_path = workspace.path().join("response.sse");
    let sse_contents = load_sse_fixture_with_id(
        "tests/fixtures/completed_template.json",
        "prompt-harness-response",
    );
    fs::write(&sse_path, sse_contents)?;

    let script_path = workspace.path().join("driver.py");
    fs::write(&script_path, driver_script())?;

    let mut cmd = Command::cargo_bin("prompt_harness")?;
    cmd.env("CODEX_HOME", &codex_home)
        .env("CODEX_RS_SSE_FIXTURE", &sse_path)
        .env("OPENAI_API_KEY", "test-key")
        .arg("--system-prompt-file")
        .arg(&prompt_path)
        .arg("python3")
        .arg(&script_path)
        .assert()
        .success();

    Ok(())
}

fn driver_script() -> String {
    r#"#!/usr/bin/env python3
import json
import sys

first = sys.stdin.readline()
if not first:
    sys.exit("missing session_configured event")

message = json.loads(first)
if message.get("msg", {}).get("type") != "session_configured":
    sys.exit("unexpected initial event type")

submission = {"id": "interrupt", "op": {"type": "interrupt"}}
print(json.dumps(submission), flush=True)
"#
    .to_string()
}
