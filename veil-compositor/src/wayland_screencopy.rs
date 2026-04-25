use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};

pub struct ScreencopyCapture {
    mmap: *mut u8,
    size: usize,
    width: u32,
    height: u32,
    stride: u32,
}

impl ScreencopyCapture {
    pub fn new(width: u32, height: u32, stride: u32) -> Option<Self> {
        let size = (height * stride) as usize;

        let fd = unsafe {
            let fd = libc::memfd_create(
                "veil_screencopy\0".as_ptr() as *const i8,
                libc::MFD_CLOEXEC,
            );
            if fd < 0 {
                return None;
            }
            OwnedFd::from_raw_fd(fd)
        };

        unsafe {
            if libc::ftruncate(fd.as_raw_fd(), size as i64) < 0 {
                return None;
            }
        }

        let mmap = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            ) as *mut u8
        };

        if mmap == libc::MAP_FAILED as *mut u8 {
            return None;
        }

        Some(Self {
            mmap,
            size,
            width,
            height,
            stride,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.mmap, self.size) }
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.mmap, self.size) }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }
}

impl Drop for ScreencopyCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::munmap(self.mmap as *mut libc::c_void, self.size);
        }
    }
}
