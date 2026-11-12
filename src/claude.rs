//! Claude Code CLI (`claude -p`) calls: meeting summaries and live answers. Only used when the user enabled it.

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(300);

const SUMMARY_PROMPT: &str = "You summarize meeting transcripts produced by automatic speech recognition. \
Lines look like '[HH:MM:SS] [Speaker] text'; speaker names and some words may be misrecognized. \
Write everything, including the section headings, in the main language of the transcript. \
Output Markdown with exactly these sections: Summary (3-6 sentences), Decisions, \
Action items (who / what / until when, if mentioned) and Open questions. \
Write 'none' (translated) for empty sections. Do not invent facts that are not in the transcript.";

pub fn find_claude() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["claude.exe", "claude.cmd", "claude.bat"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let fallback = PathBuf::from(std::env::var_os("USERPROFILE")?).join(".local").join("bin").join("claude.exe");
    fallback.is_file().then_some(fallback)
}

/// One non-interactive `claude -p` call without session, MCP servers or (by default) any tools.
pub fn run_claude(
    prompt: &str,
    system_prompt: &str,
    model: &str,
    timeout: Duration,
    cwd: &Path,
    tools: &[&str],
) -> Result<String> {
    let exe = find_claude().context("Claude Code CLI ('claude') not found. Install it or add it to PATH.")?;
    let mut cmd = Command::new(exe);
    cmd.args(["-p", "--model", model, "--no-session-persistence", "--strict-mcp-config"]);
    cmd.args(["--system-prompt", system_prompt]);
    if tools.is_empty() {
        cmd.args(["--tools", ""]);
    } else {
        let list = tools.join(",");
        cmd.args(["--tools", &list, "--allowedTools", &list]);
    }
    let mut child = cmd
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("could not start claude")?;
    let mut stdin = child.stdin.take().unwrap();
    let input = prompt.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let read = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = read(Box::new(child.stdout.take().unwrap()));
    let err = read(Box::new(child.stderr.take().unwrap()));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            bail!("claude did not answer within {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = writer.join();
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if !status.success() || stdout.trim().is_empty() {
        let msg = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
        bail!("{}", if msg.is_empty() { format!("claude exited with {status}") } else { msg.to_string() });
    }
    Ok(stdout.trim().to_string())
}

/// Runs the CLI on the transcript and writes <transcript>_summary.md next to it.
pub fn summarize(transcript: &Path, model: &str) -> Result<PathBuf> {
    let text = std::fs::read_to_string(transcript).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    anyhow::ensure!(!lines.is_empty(), "The transcript is still empty.");
    let name = transcript.file_name().unwrap_or_default().to_string_lossy().into_owned();
    log::info!("Summarizing {name} with Claude ({model}, {} lines)", lines.len());
    let answer = run_claude(
        &format!("Summarize this meeting transcript:\n\n{}", lines.join("\n")),
        SUMMARY_PROMPT,
        model,
        SUMMARY_TIMEOUT,
        transcript.parent().unwrap_or(Path::new(".")),
        &[],
    )?;
    let stem = transcript.file_stem().unwrap_or_default().to_string_lossy();
    let out = transcript.with_file_name(format!("{stem}_summary.md"));
    std::fs::write(&out, format!("# Summary of {name}\n\n{answer}\n"))?;
    log::info!("Summary written to {}", out.display());
    Ok(out)
}
