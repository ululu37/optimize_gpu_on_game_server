use bytemuck::{Pod, Zeroable};
use rand::Rng;
use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

const TARGET_TPS: f64 = 30.0;
const TEST_DURATION_SECS: u64 = 3600; 
const START_PLAYER_COUNT: u64 = 1_000_000; 
const WORKGROUP_SIZE: u32 = 256;
const RADIUS: f32 = 200.0;
const MAP_SIZE: f32 = 100_000_000.0;
const GRID_SIZE: f32 = RADIUS; 

const HASH_SIZE: usize = 1_000_003; 

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PlayerData { next: u32, id: u32, x: f32, y: f32 }

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PlayerInput { dir_x: f32, dir_y: f32, speed: f32, _pad: u32 }

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PlayerOutput { count: u32, head_node: u32 }

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct NeighborNode { other_id: u32, next_node: u32 }

enum InputAction {
    Move { id: u32, dir_x: f32, dir_y: f32, speed: f32 },
    Stop { id: u32 },
}

async fn run_benchmark(device: &wgpu::Device, queue: &wgpu::Queue, clear_pipeline: &wgpu::ComputePipeline, move_pipeline: &wgpu::ComputePipeline, stream_pipeline: &wgpu::ComputePipeline, player_count: u64) -> (f64, u32, u32, u32) {
    let p_count = player_count as usize;
    let mut rng = rand::thread_rng();
    let mut cpu_players = vec![PlayerData::default(); p_count];
    
    for i in 0..p_count {
        cpu_players[i] = PlayerData { next: 0xFFFFFFFF, id: i as u32, x: rng.gen_range(0.0..MAP_SIZE), y: rng.gen_range(0.0..MAP_SIZE) };
    }

    let players_size = (p_count * std::mem::size_of::<PlayerData>()) as wgpu::BufferAddress;
    let outputs_size = (p_count * std::mem::size_of::<PlayerOutput>()) as wgpu::BufferAddress;
    let limits = device.limits();
    
    let buffer_size = (HASH_SIZE * 8) + (p_count * 16);
    if buffer_size as u64 > limits.max_storage_buffer_binding_size as u64 { return (0.0, 0, 0, 0); }

    let spatial_nodes_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Spatial Nodes Buffer"),
        size: buffer_size as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let player_section_offset = (HASH_SIZE * 8) as wgpu::BufferAddress;
    queue.write_buffer(&spatial_nodes_buffer, player_section_offset, bytemuck::cast_slice(&cpu_players));

    let inputs_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Inputs Buffer"),
        size: (p_count * std::mem::size_of::<PlayerInput>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let outputs_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Outputs Buffer"),
        size: outputs_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let max_neighbor_nodes = 10_000_000;
    let neighbor_nodes_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Neighbor Nodes Buffer"),
        size: (max_neighbor_nodes * std::mem::size_of::<NeighborNode>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    
    let allocator_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Allocator Buffer"),
        size: 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let players_staging = [
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("PStaging0"), size: players_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("PStaging1"), size: players_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
    ];

    let outputs_staging = [
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("OStaging0"), size: outputs_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("OStaging1"), size: outputs_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
    ];

    let clear_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Clear Bind Group"),
        layout: &clear_pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: spatial_nodes_buffer.as_entire_binding() }],
    });

    let move_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Move Bind Group"),
        layout: &move_pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 1, resource: inputs_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: spatial_nodes_buffer.as_entire_binding() }, 
        ],
    });

    let stream_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Stream Bind Group"),
        layout: &stream_pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 1, resource: spatial_nodes_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: outputs_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: neighbor_nodes_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: allocator_buffer.as_entire_binding() },
        ],
    });

    let (tx, rx) = mpsc::channel::<InputAction>();
    thread::spawn(move || {
        let mut rng = rand::thread_rng();
        for i in 0..p_count as u32 {
            if tx.send(InputAction::Move { id: i, dir_x: rng.gen_range(-1.0..1.0), dir_y: rng.gen_range(-1.0..1.0), speed: 5000.0 }).is_err() { return; }
        }
        loop {
            for _ in 0..10_000 {
                let id = rng.gen_range(0..p_count as u32);
                if tx.send(InputAction::Move { id, dir_x: rng.gen_range(-1.0..1.0), dir_y: rng.gen_range(-1.0..1.0), speed: 5000.0 }).is_err() { return; }
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    let mut cpu_inputs = vec![PlayerInput::default(); p_count];
    let tick_interval = Duration::from_secs_f64(1.0 / TARGET_TPS);
    let test_duration = Duration::from_secs(TEST_DURATION_SECS);

    let mut total_work_time = Duration::ZERO;
    let mut completed_ticks = 0;
    let mut dropped_ticks = 0;
    
    let mut last_print = Instant::now();
    let mut ticks_this_sec = 0;
    let mut drops_this_sec = 0;

    let master_start = Instant::now();
    let mut next_tick_time = master_start;
    
    let mut frame_idx = 0usize;
    let mut pending_maps: Vec<Option<(mpsc::Receiver<()>, mpsc::Receiver<()>)>> = vec![None, None];

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let _ = socket.set_nonblocking(true); 
    let target_addr = "127.0.0.1:9000";

    let mut packet_buffer = vec![0u8; 8 + 4000 * 16];

    while master_start.elapsed() < test_duration {
        let now = Instant::now();
        if now >= next_tick_time {
            let work_start = Instant::now();

            while let Ok(action) = rx.try_recv() {
                match action {
                    InputAction::Move { id, dir_x, dir_y, speed } => { cpu_inputs[id as usize] = PlayerInput { dir_x, dir_y, speed, _pad: 0 }; }
                    InputAction::Stop { id } => { cpu_inputs[id as usize] = PlayerInput::default(); }
                }
            }

            queue.write_buffer(&inputs_buffer, 0, bytemuck::cast_slice(&cpu_inputs));
            queue.write_buffer(&allocator_buffer, 0, bytemuck::cast_slice(&[0u32]));

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            
            { 
                let p_workgroups = (p_count as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
                let c_workgroups = (HASH_SIZE as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
                
                { // 1. Clear Grid
                    let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                    cpass.set_pipeline(clear_pipeline);
                    cpass.set_bind_group(0, &clear_bind_group, &[]);
                    cpass.dispatch_workgroups(c_workgroups, 1, 1);
                }

                { // 2. Move & Link
                    let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                    cpass.set_pipeline(move_pipeline);
                    cpass.set_bind_group(0, &move_bind_group, &[]);
                    cpass.dispatch_workgroups(p_workgroups, 1, 1);
                }
                
                { // 3. Search Neighbors
                    let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                    cpass.set_pipeline(stream_pipeline);
                    cpass.set_bind_group(0, &stream_bind_group, &[]);
                    cpass.dispatch_workgroups(p_workgroups, 1, 1);
                }
            }
            
            let curr_buf = frame_idx % 2;
            let prev_buf = (frame_idx + 1) % 2;
            
            encoder.copy_buffer_to_buffer(&spatial_nodes_buffer, player_section_offset as u64, &players_staging[curr_buf], 0, players_size);
            encoder.copy_buffer_to_buffer(&outputs_buffer, 0, &outputs_staging[curr_buf], 0, outputs_size);
            queue.submit(Some(encoder.finish()));

            let (tx_p, rx_p) = mpsc::channel();
            players_staging[curr_buf].slice(..).map_async(wgpu::MapMode::Read, move |_v| { let _ = tx_p.send(()); });
            let (tx_o, rx_o) = mpsc::channel();
            outputs_staging[curr_buf].slice(..).map_async(wgpu::MapMode::Read, move |_v| { let _ = tx_o.send(()); });
            
            pending_maps[curr_buf] = Some((rx_p, rx_o));
            device.poll(wgpu::Maintain::Poll); 

            if frame_idx > 0 {
                if let Some((rx_p_prev, rx_o_prev)) = pending_maps[prev_buf].take() {
                    let mut p_done = false;
                    let mut o_done = false;
                    while !p_done || !o_done {
                        device.poll(wgpu::Maintain::Poll);
                        if !p_done && rx_p_prev.try_recv().is_ok() { p_done = true; }
                        if !o_done && rx_o_prev.try_recv().is_ok() { o_done = true; }
                        std::hint::spin_loop();
                    }
                    { 
                        let p_data = players_staging[prev_buf].slice(..).get_mapped_range();
                        cpu_players.copy_from_slice(bytemuck::cast_slice(&p_data)); 
                    }
                    players_staging[prev_buf].unmap();
                    outputs_staging[prev_buf].unmap();

                    let chunk_size = 4000;
                    for (i, chunk) in cpu_players.chunks(chunk_size).enumerate() {
                        let offset = (i * chunk_size) as u32;
                        let count = chunk.len() as u32;
                        let payload_len = 8 + chunk.len() * 16;
                        packet_buffer[0..4].copy_from_slice(&offset.to_le_bytes());
                        packet_buffer[4..8].copy_from_slice(&count.to_le_bytes());
                        packet_buffer[8..payload_len].copy_from_slice(bytemuck::cast_slice(chunk));
                        let _ = socket.send_to(&packet_buffer[0..payload_len], target_addr); 
                    }
                }
            }

            frame_idx += 1;
            total_work_time += work_start.elapsed();
            completed_ticks += 1;
            ticks_this_sec += 1;
            next_tick_time += tick_interval;

            while next_tick_time <= Instant::now() {
                next_tick_time += tick_interval;
                dropped_ticks += 1;
                drops_this_sec += 1;
            }

            if last_print.elapsed().as_secs_f64() >= 1.0 {
                println!("🔥 GPU Server (Linked List) | TPS: {} | Dropped/sec: {} | Active Objects: {}", ticks_this_sec, drops_this_sec, p_count);
                ticks_this_sec = 0;
                drops_this_sec = 0;
                last_print = Instant::now();
            }
        } else {
            std::hint::spin_loop();
        }
    }

    let avg_ms = if completed_ticks > 0 { (total_work_time.as_secs_f64() * 1000.0) / (completed_ticks as f64) } else { 0.0 };
    let expected = (TARGET_TPS * TEST_DURATION_SECS as f64).round() as u32;
    (avg_ms, completed_ticks, expected, dropped_ticks)
}

async fn run() {
    let instance = wgpu::Instance::default();
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.unwrap();
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor { label: None, required_features: wgpu::Features::empty(), required_limits: adapter.limits() }, None).await.unwrap();

    let clear_shader_source = format!("
        @group(0) @binding(0) var<storage, read_write> nodes: array<atomic<u32>>;
        @compute @workgroup_size(256) fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
            let h = gid.x; if (h >= {:?}u) {{ return; }}
            atomicStore(&nodes[h * 2u], 0xFFFFFFFFu); 
            atomicStore(&nodes[h * 2u + 1u], 0xFFFFFFFFu); 
        }}", HASH_SIZE);

    let move_shader_source = format!("
        struct Input {{ dx: f32, dy: f32, s: f32, p: u32 }}
        @group(0) @binding(1) var<storage, read> inputs: array<Input>;
        @group(0) @binding(2) var<storage, read_write> nodes: array<atomic<u32>>;
        @compute @workgroup_size(256) fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
            let i = gid.x; if (i >= {:?}u) {{ return; }}
            let off = {:?}u * 2u + i * 4u;
            var x = bitcast<f32>(atomicLoad(&nodes[off + 2u]));
            var y = bitcast<f32>(atomicLoad(&nodes[off + 3u]));
            let inp = inputs[i];
            if (inp.s > 0.0) {{
                x += inp.dx * inp.s * 0.0333; y += inp.dy * inp.s * 0.0333;
                let ms = {:?} - 1.0; if (x < 0.0) {{ x = ms; }} if (x > ms) {{ x = 0.0; }}
                if (y < 0.0) {{ y = ms; }} if (y > ms) {{ y = 0.0; }}
            }}
            atomicStore(&nodes[off + 2u], bitcast<u32>(x));
            atomicStore(&nodes[off + 3u], bitcast<u32>(y));
            let h = ((u32(x/{:?})*73856093u)^(u32(y/{:?})*19349663u))%{:?}u;
            let old = atomicExchange(&nodes[h * 2u + 1u], i);
            atomicStore(&nodes[off], old);
        }}", START_PLAYER_COUNT, HASH_SIZE, MAP_SIZE, GRID_SIZE, GRID_SIZE, HASH_SIZE);

    let stream_shader_source = format!("
        struct NeighborNode {{ other_id: u32, next_node: u32, }}
        struct Output {{ count: atomic<u32>, head: atomic<u32> }}
        @group(0) @binding(1) var<storage, read> nodes: array<u32>;
        @group(0) @binding(2) var<storage, read_write> outputs: array<Output>;
        @group(0) @binding(3) var<storage, read_write> neighbor_nodes: array<NeighborNode>;
        @group(0) @binding(4) var<storage, read_write> allocator: atomic<u32>;

        @compute @workgroup_size(256) fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
            let i = gid.x; if (i >= {:?}u) {{ return; }}
            let poff = {:?}u * 2u + i * 4u;
            let px = bitcast<f32>(nodes[poff + 2u]);
            let py = bitcast<f32>(nodes[poff + 3u]);
            atomicStore(&outputs[i].count, 0u);
            let cx = i32(px/{:?}); let cy = i32(py/{:?});
            let ox = select(-1, 1, (px/{:?} - f32(cx)) > 0.5);
            let oy = select(-1, 1, (py/{:?} - f32(cy)) > 0.5);
            for (var r=0; r<=1; r++) {{ for (var c=0; c<=1; c++) {{
                let nc = cx + c*ox; let nr = cy + r*oy;
                if (nc<0 || nr<0) {{ continue; }}
                let h = ((u32(nc)*73856093u)^(u32(nr)*19349663u))%{:?}u;
                var p2 = nodes[h * 2u + 1u];
                var loop_overflow = 0u;
                while (p2 != 0xFFFFFFFFu && loop_overflow < 200u) {{
                    loop_overflow++;
                    if (p2 != i) {{
                        let p2off = {:?}u * 2u + p2 * 4u;
                        let p2x = bitcast<f32>(nodes[p2off + 2u]);
                        let p2y = bitcast<f32>(nodes[p2off + 3u]);
                        if (abs(p2x-px)<=200.0 && abs(p2y-py)<=200.0) {{
                            let n_idx = atomicAdd(&allocator, 1u);
                            if (n_idx < 10000000u) {{
                                neighbor_nodes[n_idx].other_id = p2;
                                let old_h = atomicExchange(&outputs[i].head, n_idx);
                                neighbor_nodes[n_idx].next_node = old_h;
                                atomicAdd(&outputs[i].count, 1u);
                            }}
                        }}
                    }}
                    p2 = nodes[{:?}u * 2u + p2 * 4u];
                }}
            }} }}
        }}", START_PLAYER_COUNT, HASH_SIZE, GRID_SIZE, GRID_SIZE, GRID_SIZE, GRID_SIZE, HASH_SIZE, HASH_SIZE, HASH_SIZE);

    let sm_clear = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(clear_shader_source.into()) });
    let sm_move = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(move_shader_source.into()) });
    let sm_stream = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(stream_shader_source.into()) });

    let p_clear = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &sm_clear, entry_point: "main" });
    let p_move = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &sm_move, entry_point: "main" });
    let p_stream = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &sm_stream, entry_point: "main" });

    println!("\n🚀 PURE GPU ENGINE (Manual Offset Mode): Reverted to Stable Version");
    println!("{:<13} | {:<12} | {:<15} | {:<13} | {:<14}", "Player Count", "Avg Work(ms)", "Ticks (Fin/Tgt)", "Dropped Ticks", "Status");
    println!("{:-<75}", "");

    let count = START_PLAYER_COUNT;
    let (avg_ms, completed, expected, dropped) = run_benchmark(&device, &queue, &p_clear, &p_move, &p_stream, count).await;
    println!("{:<13} | {:<12.2} | {:<15} | {:<13} | {}", count, avg_ms, format!("{}/{}", completed, expected), dropped, "✅ DONE");
}

fn main() { pollster::block_on(run()); }
