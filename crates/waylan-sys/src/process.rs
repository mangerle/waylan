//! 平台操作系统外部子进程安全执行与生命周期守护模块
//!
//! # 设计原理
//! - **实现初衷**：在与宿主机桌面环境（如 GNOME Shell、IBus、GSettings）交互拉起外部命令时，
//!   若直接调用同步的 `Command::output()`，一旦 D-Bus 或桌面总线挂起，调用线程将被永久阻塞，导致
//!   输入法主事件循环直接卡死。
//! - **核心优势**：通过并发管道读取避免缓冲区打满死锁，结合非阻塞 `try_wait` 微休眠轮询，在超时发生时
//!   主动发送 `SIGKILL` 并调用 `wait` 回收进程控制块（PCB），彻底杜绝僵尸进程残留与事件循环冻结。
//! - **代价与局限**：命令执行期间存在微休眠轮询（10ms），超时场景下子进程会被强制终止。

use log::{debug, warn};
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 默认外部系统命令执行超时时限（2 秒）
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// 单个输出管道允许读取的最大字节数（1 MB）
///
/// # 设计原理
/// - **实现初衷**：子进程 stdout/stderr 属于未知体量的外部数据流，失控或冗音的输出
///   若被 `read_to_end` 无上限搬入内存会造成 OOM。
/// - **核心优势**：读取侧设上限后，管道缓冲区很快写满并反向阻塞子进程写入，
///   配合执行超时机制将其强制回收，形成有界内存占用。
/// - **代价与局限**：超出上限的尾部输出被静默截断，因此仅适用于取回短小的
///   D-Bus / GSettings 查询结果，不得用于大体积数据传输。
pub const MAX_PIPE_OUTPUT_BYTES: u64 = 1024 * 1024;

/// 管道读取工作线程句柄对
struct PipeReaderHandles {
    stdout_join: JoinHandle<Vec<u8>>,
    stderr_join: JoinHandle<Vec<u8>>,
}

/// 以有界方式读完一条管道，避免未知体量输出打满内存
fn read_pipe_bounded(pipe: Option<impl Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe.take(MAX_PIPE_OUTPUT_BYTES).read_to_end(&mut buf);
    }
    buf
}

/// 启动并发管道读取线程以防管道缓冲区打满死锁
fn spawn_pipe_readers(child: &mut Child) -> PipeReaderHandles {
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_join = thread::spawn(move || read_pipe_bounded(stdout_pipe));
    let stderr_join = thread::spawn(move || read_pipe_bounded(stderr_pipe));

    PipeReaderHandles {
        stdout_join,
        stderr_join,
    }
}

/// 等待子进程退出或超时强制回收
fn wait_child_with_timeout(
    mut child: Child,
    readers: PipeReaderHandles,
    timeout: Duration,
) -> io::Result<Output> {
    let start = Instant::now();
    let poll_interval = Duration::from_millis(10);

    let status: ExitStatus = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = readers.stdout_join.join();
                    let _ = readers.stderr_join.join();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "外部子进程执行超时（超过 {:?}），已被强制终止并回收",
                            timeout
                        ),
                    ));
                }
                thread::sleep(poll_interval);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = readers.stdout_join.join();
                let _ = readers.stderr_join.join();
                return Err(e);
            }
        }
    };

    let stdout = readers.stdout_join.join().unwrap_or_default();
    let stderr = readers.stderr_join.join().unwrap_or_default();

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// 带超时保护与自动僵尸进程回收的外部命令执行函数
///
/// # Errors
/// 若子进程派生失败、I/O 管道异常或在超时时间内未完成执行，将返回对应的 [`io::Error`]。
pub fn run_command_with_timeout(mut cmd: Command, timeout: Duration) -> io::Result<Output> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let readers = spawn_pipe_readers(&mut child);
    wait_child_with_timeout(child, readers, timeout)
}

/// 在独立后台线程中异步执行带超时保护的外部命令（不阻塞当前线程）
pub fn spawn_command_detached(cmd: Command, task_name: &'static str, timeout: Duration) {
    let builder = thread::Builder::new().name(format!("waylan-cmd-{}", task_name));
    let spawn_result = builder.spawn(move || match run_command_with_timeout(cmd, timeout) {
        Ok(output) => {
            if !output.status.success() {
                debug!(
                    "[系统子进程] 后台命令 [{}] 退出状态非成功: {:?}",
                    task_name,
                    output.status.code()
                );
            }
        }
        Err(e) => {
            debug!(
                "[系统子进程] 后台命令 [{}] 执行失败或超时: {}",
                task_name, e
            );
        }
    });

    if let Err(e) = spawn_result {
        warn!(
            "[系统子进程] 派发后台命令任务 [{}] 线程失败: {}",
            task_name, e
        );
    }
}

/// 安全读取 GSettings 键值（带超时保护）
pub fn safe_gsettings_get(schema: &str, key: &str, timeout: Duration) -> Option<String> {
    let mut cmd = Command::new("gsettings");
    cmd.args(["get", schema, key]);
    let out = run_command_with_timeout(cmd, timeout).ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// 安全设置 GSettings 键值（带超时保护）
pub fn safe_gsettings_set(schema: &str, key: &str, value: &str, timeout: Duration) -> bool {
    let mut cmd = Command::new("gsettings");
    cmd.args(["set", schema, key, value]);
    run_command_with_timeout(cmd, timeout)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 异步后台安全设置 GSettings 键值（不阻塞调用线程，带超时保护与僵尸进程回收）
pub fn safe_gsettings_set_async(schema: &str, key: &str, value: &str) {
    let mut cmd = Command::new("gsettings");
    cmd.args(["set", schema, key, value]);
    spawn_command_detached(cmd, "gsettings-set", DEFAULT_COMMAND_TIMEOUT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_command_success() {
        let mut cmd = Command::new("echo");
        cmd.arg("微澜安全子进程");
        let res = run_command_with_timeout(cmd, Duration::from_secs(1));
        assert!(res.is_ok());
        let output = res.unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("微澜安全子进程"));
    }

    #[test]
    fn test_run_command_timeout() {
        let mut cmd = Command::new("sleep");
        cmd.arg("2");
        let start = Instant::now();
        let res = run_command_with_timeout(cmd, Duration::from_millis(50));
        let elapsed = start.elapsed();

        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(elapsed < Duration::from_millis(500));
    }

    #[test]
    fn test_run_command_not_found() {
        let cmd = Command::new("definitely_non_existent_binary_waylan_test");
        let res = run_command_with_timeout(cmd, Duration::from_millis(100));
        assert!(res.is_err());
        assert_ne!(res.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}
