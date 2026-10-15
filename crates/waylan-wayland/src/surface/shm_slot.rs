use memmap2::MmapMut;
use std::fs::File;
use wayland_client::protocol::wl_shm_pool::WlShmPool;

/// 候选框共享内存持久槽位（复用匿名文件描述符、预分配内存空间、持久映射与 Wayland 共享内存池）
pub(crate) struct ShmSlot {
    /// 匿名共享内存文件
    pub(crate) file: File,
    /// 当前已向内核申请的文件容量（字节）
    pub(crate) capacity: usize,
    /// 标记当前槽位的缓冲区是否正被合成器读取占用
    pub(crate) is_busy: bool,
    /// 持久复用的 Wayland 共享内存池及其当前在合成器侧映射的字节大小（消除每次按键重复 mmap/munmap）
    pub(crate) pool: Option<(WlShmPool, usize)>,
    /// 内存映射句柄（直接写入共享内存，消除通过 file.seek 与 file.write_all 的重复用户态到内核态拷贝）
    pub(crate) mmap: Option<MmapMut>,
}

impl ShmSlot {
    /// 创建带有指定初始容量的共享内存槽位
    ///
    /// # 设计原理
    /// - **实现初衷**：为候选框表面渲染提供预分配与持久内存映射槽位，消除反复创建文件与映射开销。
    /// - **核心优势**：零拷贝内存直写，配合 4 缓冲槽位循环切换，避免锁竞争与渲染闪烁。
    /// - **代价与局限**：占用常驻内存与文件描述符，需依赖合理的槽位数量控制。
    ///
    /// # Errors
    /// 当创建底层共享内存匿名文件或初始化内存映射失败时返回系统 I/O 错误。
    pub(crate) fn new(capacity: usize) -> std::io::Result<Self> {
        let file = create_shm_fd(capacity)?;
        // SAFETY: 刚创建的匿名文件尚未与其他线程或进程共享，创建映射是安全的
        let mmap = unsafe { MmapMut::map_mut(&file).ok() };
        Ok(Self {
            file,
            capacity,
            is_busy: false,
            pool: None,
            mmap,
        })
    }

    /// 确保文件容量至少达到目标字节数（容量不足时按 2 倍扩容并重新映射）
    ///
    /// # 设计原理
    /// - **实现初衷**：应对网格大展开态与超高分辨率（4K+）大尺寸候选框渲染所需的临时大像素缓冲。
    /// - **核心优势**：指数级扩容避免频繁系统调用，优先复用已分配容量。
    /// - **代价与局限**：扩容期间需销毁旧映射并重新创建 mmap，存在微秒级短暂耗时。
    ///
    /// # Errors
    /// 当调用内核 `set_len` 调整底层文件长度失败时返回系统 I/O 错误。
    pub(crate) fn ensure_capacity(&mut self, required_bytes: usize) -> std::io::Result<()> {
        if self.capacity < required_bytes {
            let new_cap = required_bytes
                .max(self.capacity.saturating_mul(2))
                .max(64 * 1024);
            self.mmap = None;
            self.file.set_len(new_cap as u64)?;
            self.capacity = new_cap;
            // SAFETY: 当前槽位独占底层共享文件句柄，旧映射已先清空，扩容后建立新映射是安全的
            self.mmap = unsafe { MmapMut::map_mut(&self.file).ok() };
        } else if self.mmap.is_none() {
            // SAFETY: 底层文件容量满足要求且无活跃映射，重新建立可变内存映射是安全的
            self.mmap = unsafe { MmapMut::map_mut(&self.file).ok() };
        }
        Ok(())
    }
}

/// 创建用于跨进程传递给 Wayland 合成器的匿名共享内存文件
///
/// # 设计原理
/// - **实现初衷**：Wayland 共享内存协议 (`wl_shm`) 要求客户端提供文件描述符供合成器映射读取。
/// - **核心优势**：在 Linux 环境优先使用原生 `memfd_create` 系统调用在 RAM 中分配匿名文件，杜绝磁盘 I/O。
/// - **代价与局限**：在不支持 memfd 的宿主内核中需自动降级至临时文件目录。
///
/// # Errors
/// 当创建匿名内存文件或截断设定文件长度失败时返回系统 I/O 错误。
pub(crate) fn create_shm_fd(size: usize) -> std::io::Result<File> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{memfd_create, MemfdFlags};
        if let Ok(fd) = memfd_create(
            "waylan-candidate-shm",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        ) {
            let file = File::from(fd);
            file.set_len(size as u64)?;
            return Ok(file);
        }
    }
    let file = tempfile::tempfile()?;
    file.set_len(size as u64)?;
    Ok(file)
}
