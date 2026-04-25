use std::mem;
use std::os::unix::io::AsRawFd;
use std::fs::OpenOptions;

#[repr(C)]
struct CaptureHeader {
    magic: u32,
    frame_id: u64,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
}

const MAGIC: u32 = 0x5645_4C43;

pub struct ShmCapture {
    mmap_ptr: *mut u8,
    size: usize,
    last_frame_id: u64,
}

impl ShmCapture {
    pub fn open(pid: u32) -> Option<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(false)
            .open(format!("/dev/shm/veil_{}", pid))
            .ok()?;

        let fd = file.as_raw_fd();
        let size = file.metadata().ok()?.len() as usize;

        if size < mem::size_of::<CaptureHeader>() {
            return None;
        }

        let mmap_ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            ) as *mut u8
        };

        if mmap_ptr == libc::MAP_FAILED as *mut u8 {
            return None;
        }

        Some(Self {
            mmap_ptr,
            size,
            last_frame_id: 0,
        })
    }

    pub fn read_frame(&mut self) -> Option<(u32, u32, u32, Vec<u8>)> {
        if self.mmap_ptr.is_null() {
            return None;
        }

        unsafe {
            let header = &*(self.mmap_ptr as *const CaptureHeader);

            if header.magic != MAGIC || header.frame_id == self.last_frame_id {
                return None;
            }

            self.last_frame_id = header.frame_id;

            let width = header.width;
            let height = header.height;
            let stride = header.stride;
            let pixel_size = (height as usize) * (stride as usize);

            let header_size = mem::size_of::<CaptureHeader>();
            if header_size + pixel_size > self.size {
                return None;
            }

            let pixel_start = self.mmap_ptr.add(header_size);
            let pixels = std::slice::from_raw_parts(pixel_start, pixel_size).to_vec();

            Some((width, height, stride, pixels))
        }
    }
}

impl Drop for ShmCapture {
    fn drop(&mut self) {
        if !self.mmap_ptr.is_null() {
            unsafe {
                let _ = libc::munmap(self.mmap_ptr as *mut libc::c_void, self.size);
            }
        }
    }
}
