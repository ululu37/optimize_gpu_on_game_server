use bytemuck::{Pod, Zeroable};
use std::net::UdpSocket;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

const MAX_P_COUNT: usize = 2_000_000; // จองเผื่อไว้ 2 ล้านคน (ใช้ VRAM แค่ 32MB) ให้มันปรับขนาดเองได้!
const MAP_SIZE: f32 = 100_000_000.0;
const DOT_SIZE: f32 = 0.0001; // 👈 สร้างตัวแปรปรับขนาดมดให้เรียบร้อย! (ทศนิยม 0.005=ใหญ่ / 0.0005=เล็ก)

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PlayerData { next: u32, id: u32, x: f32, y: f32 }

async fn run() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    let window = Arc::new(WindowBuilder::new().with_title("MMO 1 Million Player Viewer (UDP Client)").build(&event_loop).unwrap());
    
    let instance = wgpu::Instance::default();
    let surface = instance.create_surface(window.clone()).unwrap();
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() }).await.unwrap();
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await.unwrap();

    let size = window.inner_size();
    let mut config = surface.get_default_config(&adapter, size.width, size.height).unwrap();
    surface.configure(&device, &config);

    // สร้างข้อมูลเปล่าๆ หลอก GPU ไว้เต็มพิกัด 2 ล้านตัวไปเลย 
    // และจับไปซ่อนไว้นอกจอก่อน (ป้องกันคนที่โหลดไม่ทันหรือหลุดท่อ UDP ไปกองกันเป็นจุดเดียวมุมจอ)
    let mut players = vec![PlayerData::default(); MAX_P_COUNT];
    for p in players.iter_mut() {
        p.x = -999999.0;
        p.y = -999999.0;
    }

    let players_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Players Buffer"),
        contents: bytemuck::cast_slice(&players),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, // อนุญาตให้เขียนทับได้
    });

    // ----------------------------------------------------------------------
    // UDP Receiver Thread: ดึงข้อมูลจากเซิร์ฟเวอร์ แล้วอัดลง VRAM โดยตรง!
    // ----------------------------------------------------------------------
    let queue_arc = Arc::new(queue);
    let queue_clone = queue_arc.clone();
    let pb_clone = Arc::new(players_buffer);
    let players_buffer_render = pb_clone.clone();

    let active_p_count = Arc::new(AtomicU32::new(0));
    let active_p_clone = active_p_count.clone();
    let last_recv = Arc::new(RwLock::new(std::time::Instant::now()));
    let last_recv_clone = last_recv.clone();

    std::thread::spawn(move || {
        let socket = UdpSocket::bind("127.0.0.1:9000").unwrap();
        let mut buf = vec![0u8; 65535];
        loop {
            if let Ok((size, _)) = socket.recv_from(&mut buf) {
                if size >= 8 {
                    let offset = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                    let count = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                    let payload = &buf[8..size];
                    
                    let end_idx = offset + count;
                    if end_idx as usize <= MAX_P_COUNT {
                        // WGPU Thread-safe: ยิงข้อมูลจาก Network Thread ทะลุเข้าการ์ดจอโดยตรง!
                        queue_clone.write_buffer(
                            &pb_clone,
                            (offset as u64) * 16,
                            payload
                        );
                        
                        // อัปเดตบอกว่าเราเจอผู้เล่นมากสุดตัวที่เท่าไหร่แล้วแบบ Dynamic!
                        active_p_clone.fetch_max(end_idx, Ordering::Relaxed);
                        *last_recv_clone.write().unwrap() = std::time::Instant::now();
                    }
                }
            }
        }
    });

    // ----------------------------------------------------------------------
    // Render Shader สำหรับวาดเม็ดสีล้านเม็ดบนจอตามที่เซิร์ฟเวอร์ UDP ส่งมา
    // ----------------------------------------------------------------------
    let render_wgsl = format!("
struct PlayerData {{ next: u32, id: u32, x: f32, y: f32, }}
@group(0) @binding(0) var<storage, read> players: array<PlayerData>;

struct VertexOutput {{
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
}};

@vertex
fn vs_main(@builtin(vertex_index) v_idx: u32, @builtin(instance_index) i_idx: u32) -> VertexOutput {{
    let p = players[i_idx];
    
    let half_map = {:?} / 2.0;
    let nx = (p.x - half_map) / half_map;
    let ny = (p.y - half_map) / half_map;
    
    // ย่อขนาดพิกเซลกลับลงมาเหลือ 1-2 พิกเซล ให้ดูเป็นฝุ่นมดนับล้านตัวไม่ทับกันจนล้นจอ
    let ds = {:?};
    var pos = array<vec2<f32>, 6>(
        vec2<f32>(-ds, -ds), vec2<f32>( ds, -ds), vec2<f32>(-ds,  ds),
        vec2<f32>( ds, -ds), vec2<f32>(-ds,  ds), vec2<f32>( ds,  ds),
    );
    
    let offset = pos[v_idx];
    var out: VertexOutput;
    out.clip_position = vec4<f32>(nx + offset.x, ny + offset.y, 0.0, 1.0);
    // กลับไปใช้ความโปร่งแสงต่ำๆ ให้เวลามันวิ่งทับกันเกิดเป็นคลื่นแสง Density สวยงาม
    out.color = vec4<f32>(0.0, 0.8, 1.0, 0.5); 
    return out;
}}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {{
    return in.color;
}}
", MAP_SIZE, DOT_SIZE);

    let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(render_wgsl.into()) });
    
    let render_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    });
    
    let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[&render_bind_group_layout], push_constant_ranges: &[],
    });

    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&render_pipeline_layout),
        vertex: wgpu::VertexState { module: &render_shader, entry_point: "vs_main", buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &render_shader, entry_point: "fs_main", targets: &[Some(wgpu::ColorTargetState {
            format: config.format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })]}),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
        depth_stencil: None, multisample: wgpu::MultisampleState::default(), multiview: None,
    });

    let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None, layout: &render_bind_group_layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: players_buffer_render.as_entire_binding() }],
    });

    let _ = event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => { elwt.exit(); },
            Event::WindowEvent { event: WindowEvent::Resized(new_size), .. } => {
                if new_size.width > 0 && new_size.height > 0 {
                    config.width = new_size.width; config.height = new_size.height;
                    surface.configure(&device, &config);
                }
            },
            Event::AboutToWait => { window.request_redraw(); },
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                // เซ็ตระบบลืมจำนวนผู้เล่น (กรณีเซิร์ฟดับหรือรีสตาร์ท จะได้ไม่ค้างตัวเก่า)
                if last_recv.read().unwrap().elapsed().as_millis() > 1000 {
                    active_p_count.store(0, Ordering::Relaxed);
                }
                
                let current_p_count = active_p_count.load(Ordering::Relaxed);

                // GPU Render อย่างเดียว ข้อมูลพิกัดทะลุ UDP เข้า VRAM เบื้องหลังอัตโนมัติ 100%
                if let Ok(output) = surface.get_current_texture() {
                    let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                    
                    {
                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view, resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.05, b: 0.1, a: 1.0 }), store: wgpu::StoreOp::Store },
                            })],
                            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
                        });
                        rpass.set_pipeline(&render_pipeline);
                        rpass.set_bind_group(0, &render_bind_group, &[]);
                        
                        if current_p_count > 0 {
                            rpass.draw(0..6, 0..current_p_count);
                        }
                    }
                    queue_arc.submit(Some(encoder.finish()));
                    output.present();
                }
            },
            _ => {}
        }
    });
}

fn main() { pollster::block_on(run()); }
