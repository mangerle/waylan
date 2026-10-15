use memmap2::MmapMut;
use std::fs::File;
use wayland_client::protocol::wl_shm_pool::WlShmPool;

/// 候选框共享内存持久槽位（复用匿名文件描述符、预分配内存空间、持久映射与 Wayland 共享内存池）
pub struct ShmSlot {
    /// 匿名共享内存文件
    pub file: File,
    /// 当前已向内核申请的文件容量（字节）
    pub capacity: usize,
    /// 标记当前槽位的缓冲区是否正被合成器读取占用
    pub is_busy: bool,
    /// 持久复用的 Wayland 共享内存池及其当前在合成器侧映射的字节大小（消除每次按键重复 mmap/munmap）
    pub pool: Option<(WlShmPool, usize)>,
    /// 内存映射句柄（直接写入共享内存，消除通过 file.seek 与 file.write_all 的重复用户态到内核态拷贝）
    pub mmap: Option<MmapMut>,
}

impl ShmSlot {
    /// 创建带有指定初始容量的共享内存槽位
    ///
    /// # Errors
    /// 当创建或调整共享内存文件大小失败时返回系统 I/O 错误。
    pub fn new(capacity: usize) -> std::io::Result<Self> {
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
    /// # Errors
    /// 当调整底层文件长度失败时返回系统 I/O 错误。
    pub fn ensure_capacity(&mut self, required_bytes: usize) -> std::io::Result<()> {
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
/// 在 Linux 环境优先使用纯内存文件描述符 (memfd_create)，失败时平滑降级至 tempfile
///
/// # Errors
/// 当创建匿名内存文件或截断设定文件长度失败时返回系统 I/O 错误。
pub fn create_shm_fd(size: usize) -> std::io::Result<File> {
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
