//! 语音后台常驻工作线程主循环与事件驱动
//!
//! # 设计原理
//! - **实现初衷**：将常驻工作线程的命令监听、空闲超时回收与录音会话拉起逻辑从控制器门面中剥离。
//! - **核心优势**：职责单一（SRP），主会话控制器仅负责线程句柄纳管与对外 API，
//!   后台常驻循环独立负责模型复用与音频采集管道生命周期。
//! - **代价与局限**：线程间交互依赖强类型有界通道，需显式传递原子标志与接收端。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::Instant;

use log::{info, warn};

use super::model_cache::{check_idle_timeout_and_calculate_deadline, WorkerModelCache};
use super::pipeline::active_recording_loop;
use super::types::{ActiveRecordingContext, VoiceEvent, VoiceSessionOptions, WorkerCommand};
use crate::capture::AudioCapture;
use crate::vad::WaveformVisualizer;

/// 语音后台常驻工作循环持久状态
struct VoiceWorkerLoopState<'a> {
    cache: &'a mut WorkerModelCache,
    visualizer: &'a mut WaveformVisualizer,
    cmd_rx: &'a Receiver<WorkerCommand>,
    is_active: &'a Arc<AtomicBool>,
}

/// 常驻工作线程主入口：纳管模型单例生命周期与会话状态机
pub(crate) fn run_worker_main(cmd_rx: Receiver<WorkerCommand>, is_active: Arc<AtomicBool>) {
    let mut visualizer = WaveformVisualizer::new();
    let mut cache = WorkerModelCache::default();
    let mut last_activity_time = Instant::now();

    info!("[语音会话] 后台常驻语音工作线程启动完成，进入就绪待命状态");

    loop {
        let timeout = check_idle_timeout_and_calculate_deadline(&mut cache, last_activity_time);
        let cmd = match cmd_rx.recv_timeout(timeout) {
            Ok(c) => c,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        last_activity_time = Instant::now();
        match cmd {
            WorkerCommand::Start { options, event_tx } => {
                let mut loop_state = VoiceWorkerLoopState {
                    cache: &mut cache,
                    visualizer: &mut visualizer,
                    cmd_rx: &cmd_rx,
                    is_active: &is_active,
                };
                if handle_start_command(&mut loop_state, *options, event_tx) {
                    info!("[语音会话] 录音过程中收到销毁指令，后台常驻语音工作线程平滑退出");
                    break;
                }
            }
            WorkerCommand::StopAndCommit | WorkerCommand::Cancel => {
                is_active.store(false, Ordering::Release);
            }
            WorkerCommand::UnloadModels => {
                cache.unload();
                info!("[语音会话] 已卸载内存中缓存的语音识别、VAD与标点模型权重，完全释放常驻内存");
            }
            WorkerCommand::Shutdown => {
                info!("[语音会话] 收到销毁指令，后台常驻语音工作线程平滑退出");
                break;
            }
        }
    }

    is_active.store(false, Ordering::Release);
}

/// 处理 Start 指令并拉起活动录音事件循环，返回是否收到了全局销毁指令需要退出常驻工作线程
fn handle_start_command(
    state: &mut VoiceWorkerLoopState<'_>,
    options: VoiceSessionOptions,
    event_tx: SyncSender<VoiceEvent>,
) -> bool {
    let Some((recognizer, vad, punct_restorer)) = state.cache.prepare_models(&options.voice_config)
    else {
        warn!("[语音会话] 语音识别器或 VAD 模型未正确初始化，跳过本次录音");
        state.is_active.store(false, Ordering::Release);
        return false;
    };
    state.visualizer.reset();

    let (pcm_tx, pcm_rx) = sync_channel::<Vec<f32>>(32);
    let capture = match AudioCapture::start_with_device(
        pcm_tx,
        options.voice_config.device_name.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            warn!("[语音会话] 启动音频采集硬件失败: {}", e);
            let _ = event_tx.send(VoiceEvent::Error(e.to_string()));
            state.is_active.store(false, Ordering::Release);
            return false;
        }
    };

    info!("[语音会话] 录音捕获流与识别管道启动就绪，正在聆听用户语音...");
    active_recording_loop(ActiveRecordingContext {
        capture: Some(capture),
        pcm_rx,
        cmd_rx: state.cmd_rx,
        event_tx: &event_tx,
        visualizer: state.visualizer,
        vad,
        recognizer,
        punct_restorer,
        voice_config: &options.voice_config,
        is_active: state.is_active,
    })
}
