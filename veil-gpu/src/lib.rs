use std::borrow::Cow;
use wgpu::util::DeviceExt;

const HB_SHADER:   &str = include_str!("shaders/halfblock.wgsl");
const LUMA_SHADER: &str = include_str!("shaders/luma.wgsl");

pub struct GpuEncoder {
    device:       wgpu::Device,
    queue:        wgpu::Queue,
    hb_pipeline:  wgpu::ComputePipeline,
    luma_pipeline: wgpu::ComputePipeline,
}

impl GpuEncoder {
    /// Initialise the GPU encoder on the highest-performance Vulkan device.
    /// Returns `None` if no Vulkan adapter is available.
    pub fn new() -> Option<Self> {
        pollster::block_on(Self::init())
    }

    async fn init() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends:                 wgpu::Backends::VULKAN,
            flags:                    wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options:          wgpu::BackendOptions::default(),
            display:                  None,
        });

        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference:       wgpu::PowerPreference::HighPerformance,
            compatible_surface:     None,
            force_fallback_adapter: false,
        }).await.ok()?;

        eprintln!("[veil-gpu] {}", adapter.get_info().name);

        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                label:                None,
                required_features:    wgpu::Features::empty(),
                required_limits:      wgpu::Limits::default(),
                memory_hints:         wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace:                wgpu::Trace::Off,
            },
        ).await.ok()?;

        let hb_pipeline   = make_pipeline(&device, HB_SHADER,   "halfblock");
        let luma_pipeline = make_pipeline(&device, LUMA_SHADER, "luma");

        Some(Self { device, queue, hb_pipeline, luma_pipeline })
    }

    /// GPU halfblock encode: upload RGBA → compute shader samples top/bot
    /// pixel pairs per cell → returns ColorCell vec ready for emit_halfblocks.
    pub fn encode_halfblock(
        &self,
        rgba:  &[u8],
        src_w: u32,
        src_h: u32,
        cols:  u16,
        rows:  u16,
    ) -> Vec<veil_render::ColorCell> {
        let n     = cols as u64 * rows as u64;
        let bytes = n * 8; // 2 × u32 per cell

        let (_, view) = upload_texture(&self.device, &self.queue, rgba, src_w, src_h);
        let params    = params_buf(&self.device, src_w, src_h, cols as u32, rows as u32);
        let out_buf   = storage_buf(&self.device, bytes, "hb-out");
        let staging   = staging_buf(&self.device, bytes, "hb-stage");

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:  Some("hb"),
            layout: &self.hb_pipeline.get_bind_group_layout(0),
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&view)),
                entry(1, out_buf.as_entire_binding()),
                entry(2, params.as_entire_binding()),
            ],
        });

        let mut enc = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("hb") });
        {
            let mut pass = enc.begin_compute_pass(
                &wgpu::ComputePassDescriptor { label: Some("hb"), timestamp_writes: None });
            pass.set_pipeline(&self.hb_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups((cols as u32).div_ceil(8), (rows as u32).div_ceil(8), 1);
        }
        enc.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, bytes);
        self.queue.submit([enc.finish()]);

        let raw = readback(&self.device, &staging, bytes);
        let u32s: &[u32] = bytemuck::cast_slice(&raw);
        (0..n as usize).map(|i| {
            let w0 = u32s[i * 2];
            let w1 = u32s[i * 2 + 1];
            veil_render::ColorCell {
                fg: [byte(w0, 0), byte(w0, 1), byte(w0, 2)],
                bg: [byte(w1, 0), byte(w1, 1), byte(w1, 2)],
            }
        }).collect()
    }

    /// GPU luma encode: upload RGBA → compute shader computes Rec.601 luma
    /// per cell → returns luma vec ready for luma_to_chars / apply_hysteresis.
    pub fn encode_luma(
        &self,
        rgba:  &[u8],
        src_w: u32,
        src_h: u32,
        cols:  u16,
        rows:  u16,
    ) -> Vec<u8> {
        let n     = cols as u64 * rows as u64;
        let bytes = n * 4; // 1 × u32 per cell

        let (_, view) = upload_texture(&self.device, &self.queue, rgba, src_w, src_h);
        let params    = params_buf(&self.device, src_w, src_h, cols as u32, rows as u32);
        let out_buf   = storage_buf(&self.device, bytes, "luma-out");
        let staging   = staging_buf(&self.device, bytes, "luma-stage");

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:  Some("luma"),
            layout: &self.luma_pipeline.get_bind_group_layout(0),
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&view)),
                entry(1, out_buf.as_entire_binding()),
                entry(2, params.as_entire_binding()),
            ],
        });

        let mut enc = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("luma") });
        {
            let mut pass = enc.begin_compute_pass(
                &wgpu::ComputePassDescriptor { label: Some("luma"), timestamp_writes: None });
            pass.set_pipeline(&self.luma_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups((cols as u32).div_ceil(8), (rows as u32).div_ceil(8), 1);
        }
        enc.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, bytes);
        self.queue.submit([enc.finish()]);

        let raw = readback(&self.device, &staging, bytes);
        let u32s: &[u32] = bytemuck::cast_slice(&raw);
        u32s.iter().map(|&v| (v & 0xFF) as u8).collect()
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn upload_texture(
    device: &wgpu::Device,
    queue:  &wgpu::Queue,
    rgba:   &[u8],
    w: u32,
    h: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label:           Some("frame"),
            size:            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       wgpu::TextureDimension::D2,
            format:          wgpu::TextureFormat::Rgba8Unorm,
            usage:           wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats:    &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        rgba,
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

fn params_buf(device: &wgpu::Device, src_w: u32, src_h: u32, cols: u32, rows: u32) -> wgpu::Buffer {
    let data: [u32; 4] = [src_w, src_h, cols, rows];
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label:    Some("params"),
        contents: bytemuck::cast_slice(&data),
        usage:    wgpu::BufferUsages::UNIFORM,
    })
}

fn storage_buf(device: &wgpu::Device, size: u64, label: &'static str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn staging_buf(device: &wgpu::Device, size: u64, label: &'static str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn readback(device: &wgpu::Device, staging: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    if rx.recv().ok().and_then(|r| r.ok()).is_none() {
        return vec![0u8; size as usize];
    }
    let data = slice.get_mapped_range();
    let out  = data.to_vec();
    drop(data);
    staging.unmap();
    out
}

fn entry(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

fn byte(word: u32, shift: u32) -> u8 {
    ((word >> (shift * 8)) & 0xFF) as u8
}

fn make_pipeline(device: &wgpu::Device, wgsl: &str, label: &str) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label:  Some(label),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(wgsl)),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label:               Some(label),
        layout:              None, // auto-layout from shader reflection
        module:              &module,
        entry_point:         Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache:               None,
    })
}
