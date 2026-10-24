//! Freed memory handed back to the system. glibc's allocator keeps memory a program frees for reuse,
//! per thread, and with RapidOne's hundreds of threads and photo-sized buffers that can stay held
//! long after the work is done (a session reached 28 GB, 24 Oct, where a fresh start sits at 2 GB).
//! So every half minute, and after each exported photo, the free memory goes back
//! (`malloc_trim`). Nothing rendered changes. Windows' and macOS's allocators return memory
//! themselves, so elsewhere this does nothing.

/// Hand freed memory back now.
pub fn trim() {
    #[cfg(target_os = "linux")]
    // SAFETY: malloc_trim only releases memory the allocator already holds free; it takes no pointers.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// The half-minute trim, on a thread of its own for the app's life.
pub fn start() {
    #[cfg(target_os = "linux")]
    std::thread::Builder::new()
        .name("memtrim".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            trim();
        })
        .ok();
}
