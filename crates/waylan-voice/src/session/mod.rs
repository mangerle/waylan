//! 语音输入生命周期会话控制器
//!
//! # 设计原理
//! - **实现初衷**：统一纳管音频捕获流、端点静音检测、Unicode 波形生成与识别引擎管道。
//! - **核心优势**：神经网络模型（Paraformer 与 Silero VAD）在后台专用工作线程常驻复用，避免每次录音重复加载
//!   高达 200MB 的模型权重文件；消除近 1 秒的冷启动延迟，实现快捷键按下后 0 延迟秒开录音与识别。
//! - **代价与局限**：后台常驻线程需通过命令通道安全纳管状态流转，并在 Drop 时平滑停机。

mod commit;
mod model_cache;
mod pipeline;
mod types;
mod worker_loop;

pub use types::{VoiceEvent, VoiceSessionOptions};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use log::warn;

use self::types::WorkerCommand;
use self::worker_loop::run_worker_main;

use crate::error::VoiceError;

/// 语音会话状态纳管器（长生命周期常驻控制器）
pub struct VoiceSessionController {
    /// 录音活动标志（供外界高频原子查询）
    is_active: Arc<AtomicBool>,
    /// 常驻工作线程停机请求标志（跨线程可靠传递销毁意图）
    shutdown_requested: Arc<AtomicBool>,
    /// 命令发送端
    cmd_tx: SyncSender<WorkerCommand>,
    /// 后台常驻工作线程句柄
    worker_handle: Option<JoinHandle<()>>,
}

impl Default for VoiceSessionController {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceSessionController {
    /// 尝试创建会话控制器并拉起常驻工作线程
    ///
    /// # Errors
    /// 当系统底层线程资源耗尽或创建工作线程失败时返回错误。
    pub fn try_new() -> Result<Self, VoiceError> {
        let is_active = Arc::new(AtomicBool::new(false));
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = sync_channel::<WorkerCommand>(16);

        let active_clone = Arc::clone(&is_active);
        let shutdown_clone = Arc::clone(&shutdown_requested);
        let handle = thread::Builder::new()
            .name("waylan-voice-worker".to_string())
            .spawn(move || {
                run_worker_main(cmd_rx, active_clone, shutdown_clone);
            })
            .map_err(|e| VoiceError::WorkerThreadSpawnFailed {
                message: e.to_string(),
            })?;

        Ok(Self {
            is_active,
            shutdown_requested,
            cmd_tx,
            worker_handle: Some(handle),
        })
    }

    /// 创建会话控制器并拉起常驻工作线程
    ///
    /// # 容错降级
    /// 若系统资源紧张导致后台工作线程派生失败，记录错误日志并安全降级为禁用语音功能，
    /// 保证输入法核心守护进程不会意外崩溃。
    pub fn new() -> Self {
        Self::try_new().unwrap_or_else(Self::without_worker)
    }

    /// 构造未拉起后台工作线程的语音控制器（线程资源耗尽时的安全降级形态）
    ///
    /// # 设计原理
    /// - **实现初衷**：语音属于可选增强能力，线程派生失败绝不可拖垮输入法主进程，
    ///   必须降级为一个"永不可用"的控制器实例而非返回 `Option` 让每个调用点各自判空。
    /// - **核心优势**：`shutdown_requested` 预置为 true 且 `worker_handle` 为空，
    ///   使所有录音与查询入口在既有守卫下自然短路，无需额外分支。
    /// - **代价与局限**：保留一个仅用于类型一致的命令发送端，其后续发送会被立即丢弃。
    fn without_worker(err: VoiceError) -> Self {
        log::error!(
            "[微澜语音] 创建后台常驻语音工作线程失败，已安全降级为禁用语音功能: {}",
            err
        );
        let (cmd_tx, _cmd_rx) = sync_channel::<WorkerCommand>(1);
        Self {
            is_active: Arc::new(AtomicBool::new(false)),
            shutdown_requested: Arc::new(AtomicBool::new(true)),
            cmd_tx,
            worker_handle: None,
        }
    }

    /// 当前是否正处于录音识别中
    pub fn is_recording(&self) -> bool {
        self.worker_handle.is_some() && self.is_active.load(Ordering::Acquire)
    }

    /// 启动语音录制与识别会话
    ///
    /// # 参数
    /// - `options`: 会话配置参数；
    /// - `event_tx`: 向输入法核心发送事件的有界通道。
    pub fn start(
        &mut self,
        options: VoiceSessionOptions,
        event_tx: SyncSender<VoiceEvent>,
    ) -> Result<(), VoiceError> {
        if self.worker_handle.is_none() {
            return Err(VoiceError::WorkerThreadSpawnFailed {
                message: "语音后台工作线程未就绪（已被安全降级禁用）".to_string(),
            });
        }
        if self.shutdown_requested.load(Ordering::Acquire) {
            return Err(VoiceError::WorkerThreadSpawnFailed {
                message: "语音后台工作线程正在退出，无法开始新的录音".to_string(),
            });
        }

        if self.is_recording() {
            warn!("[语音会话] 录音已在进行中，忽略重复启动请求");
            return Ok(());
        }

        self.is_active.store(true, Ordering::Release);
        if let Err(e) = self.cmd_tx.send(WorkerCommand::Start {
            options: Box::new(options),
            event_tx,
        }) {
            self.is_active.store(false, Ordering::Release);
            return Err(VoiceError::StreamStartFailed {
                message: format!("向语音工作线程发送启动指令失败: {}", e),
            });
        }

        Ok(())
    }

    /// 停止录音并请求最终提交识别结果
    pub fn stop_and_commit(&mut self) {
        if self.worker_handle.is_some() && self.is_recording() {
            // 必须先确保停止指令进入有界命令队列，再由工作线程消费后复位录音状态。
            // 若先复位 is_active，工作线程可能永远看不到 StopAndCommit，导致最终识别结果丢失。
            if let Err(e) = self.cmd_tx.send(WorkerCommand::StopAndCommit) {
                warn!("[语音会话] 发送停止并提交指令失败: {}", e);
            }
            self.is_active.store(false, Ordering::Release);
        }
    }

    /// 取消当前录音并丢弃未完成的识别
    pub fn cancel(&mut self) {
        if self.worker_handle.is_some() && self.is_recording() {
            if let Err(e) = self.cmd_tx.send(WorkerCommand::Cancel) {
                warn!("[语音会话] 发送取消指令失败: {}", e);
            }
            self.is_active.store(false, Ordering::Release);
        }
    }

    /// 卸载并释放常驻工作线程中已加载的全部模型权重，归还系统内存
    pub fn unload_models(&self) {
        if self.worker_handle.is_some() {
            let _ = self.cmd_tx.send(WorkerCommand::UnloadModels);
        }
    }

    /// 请求平滑终止后台常驻语音工作线程并等待线程资源完全回收（具备幂等性）
    ///
    /// # 设计原理
    /// - **实现初衷**：防止控制器析构时常驻工作线程沦为脱缰孤儿任务（Orphan Task）。
    /// - **核心优势**：先通过非阻塞 `try_send` 派发 `Shutdown` 指令（杜绝通道积压导致的析构死锁），
    ///   随后主动等待线程句柄 `join()` 彻底析构，确保 RAII 确定性资源释放与 Panic 感知。
    pub fn shutdown(&mut self) {
        self.is_active.store(false, Ordering::Release);
        self.shutdown_requested.store(true, Ordering::Release);
        if let Some(handle) = self.worker_handle.take() {
            // 命令通道可能已满，不能依赖 try_send 一定能送达；
            // 工作线程会周期性检查 shutdown_requested 原子标志，确保 join 不会长期阻塞。
            let _ = self.cmd_tx.try_send(WorkerCommand::Shutdown);
            if let Err(e) = handle.join() {
                log::error!("[微澜语音] 后台常驻语音工作线程析构异常 (Panic): {:?}", e);
            } else {
                log::info!("[微澜语音] 后台常驻语音工作线程已确定性平滑退出并完成资源回收");
            }
        }
    }
}

impl Drop for VoiceSessionController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use self::pipeline::{
        extract_final_text, handle_stop_and_commit, process_recognition_feed,
        push_pre_speech_silence,
    };
    use self::types::{PRE_SPEECH_BUFFER_CAPACITY, RecognitionFeedContext, StopCommitContext};
    use super::*;
    use crate::afe::AudioFrontEndPipeline;
    use crate::recognizer::{MockVoiceRecognizer, VoiceRecognizer};
    use std::collections::VecDeque;
    use waylan_core::VoiceConfig;

    #[test]
    fn test_voice_session_controller_degraded_mode() {
        // 手动构造一个工作线程派生失败时的降级控制器实例
        let (cmd_tx, _cmd_rx) = sync_channel::<WorkerCommand>(1);
        let mut controller = VoiceSessionController {
            is_active: Arc::new(AtomicBool::new(false)),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            cmd_tx,
            worker_handle: None,
        };

        // 验证处于降级模式时的各项接口安全表现
        assert!(
            !controller.is_recording(),
            "降级状态下录音状态应始终为 false"
        );

        let (event_tx, _event_rx) = sync_channel::<VoiceEvent>(4);
        let start_res = controller.start(VoiceSessionOptions::default(), event_tx);
        assert!(
            start_res.is_err(),
            "后台线程缺失时 start 应返回安全降级错误"
        );

        // 验证 stop 和 cancel 操作在降级模式下安全无副作用、不引发 panic
        controller.stop_and_commit();
        controller.cancel();
    }

    #[test]
    fn test_push_pre_speech_silence_capacity_limit() {
        let mut buffer = VecDeque::new();
        let chunk1 = vec![0.1; 2000];
        push_pre_speech_silence(&mut buffer, &chunk1);
        assert_eq!(buffer.len(), 2000);

        let chunk2 = vec![0.2; 4000];
        push_pre_speech_silence(&mut buffer, &chunk2);
        // 总计 6000 采样，超过 4800，应被限制在 PRE_SPEECH_BUFFER_CAPACITY (4800)
        assert_eq!(buffer.len(), PRE_SPEECH_BUFFER_CAPACITY);
    }

    #[test]
    fn test_process_recognition_feed_drains_buffer() {
        let mut recognizer = MockVoiceRecognizer::new();
        let mut pre_buffer = VecDeque::from(vec![0.05; 1000]);
        let mut last_partial = String::new();
        let (tx, _rx) = sync_channel(16);

        let chunk = vec![0.1; 500];
        let res = process_recognition_feed(
            RecognitionFeedContext {
                recognizer: &mut recognizer,
                pre_speech_buffer: &mut pre_buffer,
                last_partial_text: &mut last_partial,
                event_tx: &tx,
                real_time_preview: true,
            },
            &chunk,
        );

        assert!(res);
        // 预缓冲区应被全部取走注入识别器
        assert!(pre_buffer.is_empty());
    }

    #[test]
    fn test_extract_final_text_punctuation() {
        let itn_cfg = waylan_core::VoiceItnConfig::default();
        let mut recognizer = MockVoiceRecognizer::new();
        let text = extract_final_text(&mut recognizer, true, &itn_cfg);
        assert_eq!(text, "微澜语音输入");

        // 模拟自带标点的识别器输出验证自动标点与滤除标点
        struct PunctMockRecognizer;
        impl VoiceRecognizer for PunctMockRecognizer {
            fn feed_pcm(
                &mut self,
                _samples: &[f32],
            ) -> Result<Option<String>, crate::error::VoiceError> {
                Ok(None)
            }
            fn finish(&mut self) -> Result<String, crate::error::VoiceError> {
                Ok("你好，微澜语音输入！".to_string())
            }
            fn reset(&mut self) {}
        }

        let mut punct_rec = PunctMockRecognizer;
        let with_punct = extract_final_text(&mut punct_rec, true, &itn_cfg);
        assert_eq!(with_punct, "你好，微澜语音输入！");

        let mut punct_rec2 = PunctMockRecognizer;
        let stripped = extract_final_text(&mut punct_rec2, false, &itn_cfg);
        assert_eq!(stripped, "你好微澜语音输入");
    }

    #[test]
    fn test_voice_session_controller_unload_models() {
        let (cmd_tx, cmd_rx) = sync_channel::<WorkerCommand>(4);
        let dummy_handle = thread::spawn(|| {});
        let controller = VoiceSessionController {
            is_active: Arc::new(AtomicBool::new(false)),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            cmd_tx,
            worker_handle: Some(dummy_handle),
        };

        controller.unload_models();
        // 验证通道已收到 UnloadModels 指令
        let received = cmd_rx.try_recv();
        assert!(matches!(received, Ok(WorkerCommand::UnloadModels)));
    }

    #[test]
    fn test_handle_stop_and_commit_drains_audio_and_flushes_silence() {
        let (pcm_tx, pcm_rx) = sync_channel::<Vec<f32>>(16);
        // 模拟松开按键瞬间，管道中积压了 2 帧有效音频
        pcm_tx.send(vec![0.1; 1600]).unwrap();
        pcm_tx.send(vec![0.2; 1600]).unwrap();

        let (event_tx, event_rx) = sync_channel::<VoiceEvent>(16);
        let mut recognizer = MockVoiceRecognizer::new();
        let voice_cfg = VoiceConfig::default();
        let mut afe = AudioFrontEndPipeline::new(&voice_cfg.afe);
        let mut pre_speech = VecDeque::new();
        let mut last_partial = String::new();
        let is_active = Arc::new(AtomicBool::new(true));

        let ctx = StopCommitContext {
            capture: None,
            pcm_rx: &pcm_rx,
            recognizer: &mut recognizer,
            afe: &mut afe,
            pre_speech_buffer: &mut pre_speech,
            last_partial_text: &mut last_partial,
            voice_config: &voice_cfg,
            event_tx: &event_tx,
            is_active: &is_active,
        };

        handle_stop_and_commit(ctx);

        // 验证通道内积压音频已被彻底排空
        assert!(pcm_rx.try_recv().is_err(), "音频通道中的切片应已被全部排空");

        // 验证活跃标志已安全复位
        assert!(!is_active.load(Ordering::Acquire));

        // 验证成功收到包含文本的 FinalCommit 事件
        let mut final_commit_received = false;
        while let Ok(evt) = event_rx.try_recv() {
            if let VoiceEvent::FinalCommit(text) = evt {
                assert!(!text.is_empty(), "结算文本不应为空");
                final_commit_received = true;
            }
        }
        assert!(final_commit_received, "必须收到 FinalCommit 终态事件");
    }

    #[test]
    fn test_voice_session_controller_shutdown_without_deadlock() {
        // 构造控制器并在常驻工作线程中模拟收到 Shutdown 命令平滑退场
        let controller = VoiceSessionController::try_new();
        if let Ok(mut c) = controller {
            assert!(!c.is_recording());
            // 显式主动停机
            let start = std::time::Instant::now();
            c.shutdown();
            assert!(
                start.elapsed() < std::time::Duration::from_secs(2),
                "空闲语音工作线程的析构必须在 2 秒内完成"
            );
            assert!(!c.is_recording());
            // 验证多次 shutdown 具备幂等性
            c.shutdown();
            // drop 发生时自动委托 shutdown，测试其在微秒内顺利退场绝不死锁
            drop(c);
        }
    }

    #[test]
    fn test_stop_and_commit_command_reaches_worker() {
        let (cmd_tx, cmd_rx) = sync_channel::<WorkerCommand>(4);
        let (seen_tx, seen_rx) = sync_channel::<()>(1);
        let is_active = Arc::new(AtomicBool::new(true));
        let worker = thread::spawn(move || {
            if let Ok(WorkerCommand::StopAndCommit) = cmd_rx.recv() {
                let _ = seen_tx.send(());
            }
            // 模拟工作线程后续退出，避免控制器 Drop 时 join 永久阻塞
            let _ = cmd_rx.recv();
        });

        let mut controller = VoiceSessionController {
            is_active,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            cmd_tx,
            worker_handle: Some(worker),
        };
        controller.stop_and_commit();
        assert!(
            seen_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok(),
            "停止录音时必须先确保 StopAndCommit 命令送达工作线程"
        );
        controller.shutdown();
    }
}
