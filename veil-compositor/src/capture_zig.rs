use std::ffi::c_uint;

#[repr(C)]
pub struct VeilFrameHeader {
    pub magic: u32,
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub timestamp: u64,
}

#[repr(C)]
pub struct VeilFrameRing {
    _opaque: [u8; 0],
}

// TODO: Enable once build.rs is wired to compile Zig
/*
#[link(name = "veil_capture")]
extern "C" {
    // Frame conversion
    fn veil_xrgb_to_rgba(src: *const u8, src_len: usize, width: u32, height: u32, out: *mut u8) -> bool;
    fn veil_crop_rgba(src: *const u8, src_len: usize, src_w: u32, src_h: u32,
                      x: i32, y: i32, w: u32, h: u32, scale: i32,
                      out: *mut u8) -> u32;

    // SHM ring buffer
    fn veil_shm_ring_init(mmap_ptr: *mut u8, mmap_size: usize, width: u32, height: u32) -> *mut VeilFrameRing;
    fn veil_shm_ring_write(ring: *mut VeilFrameRing, rgba: *const u8, len: usize, width: u32, height: u32, ts: u64) -> bool;
    fn veil_shm_ring_read(ring: *mut VeilFrameRing, out_hdr: *mut VeilFrameHeader, out_data: *mut *mut u8) -> u32;
}
*/

// TODO: Enable once Zig extern block is uncommented
/*
/// Convert XRGB8888 (wl_shm LE) to RGBA8888.
pub fn xrgb_to_rgba(src: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0u8; src.len()];
    unsafe {
        veil_xrgb_to_rgba(src.as_ptr(), src.len(), width, height, out.as_mut_ptr());
    }
    out
}

/// Crop RGBA to a window region (logical coords * scale = physical).
pub fn crop_rgba(src: &[u8], src_w: u32, src_h: u32,
                 x: i32, y: i32, w: u32, h: u32, scale: i32,
) -> Vec<u8> {
    let estimate = (w as usize) * (h as usize) * (scale as usize) * (scale as usize) * 4;
    let mut out = vec![0u8; estimate];
    let len = unsafe {
        veil_crop_rgba(src.as_ptr(), src.len(), src_w, src_h,
                      x, y, w, h, scale,
                      out.as_mut_ptr())
    };
    out.truncate(len as usize);
    out
}

/// SHM frame ring buffer wrapper.
pub struct ShmRing {
    inner: *mut VeilFrameRing,
}

impl ShmRing {
    pub fn new(mmap_ptr: *mut u8, size: usize, width: u32, height: u32) -> Option<Self> {
        unsafe {
            let ring = veil_shm_ring_init(mmap_ptr, size, width, height);
            if ring.is_null() {
                None
            } else {
                Some(Self { inner: ring })
            }
        }
    }

    pub fn write_frame(&mut self, rgba: &[u8], width: u32, height: u32, ts: u64) -> bool {
        unsafe {
            veil_shm_ring_write(self.inner, rgba.as_ptr(), rgba.len(), width, height, ts)
        }
    }

    pub fn read_frame(&self) -> Option<(VeilFrameHeader, Vec<u8>)> {
        unsafe {
            let mut hdr = std::mem::zeroed::<VeilFrameHeader>();
            let mut data_ptr = std::ptr::null_mut();
            let len = veil_shm_ring_read(self.inner, &mut hdr, &mut data_ptr);
            if len == 0 || data_ptr.is_null() {
                return None;
            }
            let data = std::slice::from_raw_parts(data_ptr, len as usize).to_vec();
            Some((hdr, data))
        }
    }
}
*/
