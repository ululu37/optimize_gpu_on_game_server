use rayon::prelude::*;
use std::time::Instant;
use std::collections::HashMap;

const MAP_SIZE: f32 = 10000.0;
const VIEW_RADIUS: f32 = 50.0;
const MAX_STREAM: usize = 50; 
const CELL_SIZE: f32 = 50.0;
const TARGET_TPS: usize = 30;

#[derive(Clone, Copy)]
struct PlayerData {
    id: u32,
    x: f32,
    y: f32,
}

/// Buffer แบบ Stack (ทำงานแบบไม่จอง RAM ใน Heap เลยสัก byte เดียว)
#[derive(Clone, Copy)]
struct StreamBuffer {
    data: [PlayerData; MAX_STREAM],
    len: usize,
}

impl StreamBuffer {
    fn new() -> Self {
        Self {
            data: [PlayerData { id: 0, x: 0.0, y: 0.0 }; MAX_STREAM],
            len: 0,
        }
    }

    #[inline(always)]
    fn push(&mut self, p: PlayerData) -> bool {
        if self.len < MAX_STREAM {
            self.data[self.len] = p;
            self.len += 1;
            true
        } else {
            false
        }
    }
}

/// =====================================================
/// Layer 2: Grid Node 
/// =====================================================
struct GridNode {
    players: Vec<PlayerData>,
}

impl GridNode {
    fn new() -> Self {
        Self { players: Vec::new() }
    }

    #[inline(always)]
    fn insert(&mut self, player: PlayerData) {
        self.players.push(player);
    }

    fn clear(&mut self) {
        self.players.clear();
    }

    #[inline(always)]
    fn query_neighbors(&self, center_x: f32, center_y: f32, view_range: f32, exclude_id: u32, buf: &mut StreamBuffer) {
        for p in &self.players {
            if p.id == exclude_id { continue; }
            
            // หาขอบเขตการรับรู้แบบ 4 เหลี่ยม (Bounding Box)
            // เช็คว่าระยะห่างในแกน X และ Y ไม่เกินรัศมีการมองเห็น
            let dx = (p.x - center_x).abs();
            let dy = (p.y - center_y).abs();
            
            if dx <= view_range && dy <= view_range {
                if !buf.push(*p) {
                    break; // ถ้าครบ 50 คนแล้ว เลิกหางานใน Grid นี้ทันที
                }
            }
        }
    }
}


/// =====================================================
/// Layer 1: Router
/// =====================================================
struct Router {
    grids: HashMap<(usize, usize), GridNode>,
}

impl Router {
    fn new() -> Self {
        Self {
            grids: HashMap::new(),
        }
    }

    #[inline(always)]
    fn get_grid_coord(&self, x: f32, y: f32) -> (usize, usize) {
        let cx = ((x / CELL_SIZE) as usize).max(0);
        let cy = ((y / CELL_SIZE) as usize).max(0);
        (cx, cy)
    }

    fn update_player_location(&mut self, player: PlayerData) {
        let coord = self.get_grid_coord(player.x, player.y);
        self.grids.entry(coord).or_insert_with(GridNode::new).insert(player);
    }

    fn flush(&mut self) {
        self.grids.par_iter_mut().for_each(|(_, g)| g.clear());
    }

    #[inline(always)]
    fn stream_data_for(&self, player: &PlayerData) -> StreamBuffer {
        let mut buf = StreamBuffer::new();
        let (cx, cy) = self.get_grid_coord(player.x, player.y);

        let min_x = cx.saturating_sub(1);
        let max_x = cx.saturating_add(1);
        let min_y = cy.saturating_sub(1);
        let max_y = cy.saturating_add(1);

        let view_range = VIEW_RADIUS;

        'search: for ny in min_y..=max_y {
            for nx in min_x..=max_x {
                // ค้นหา Map: หาก Zone x,y ไหนมีอยู่จริง ก็ดึงออกมา
                if let Some(grid_node) = self.grids.get(&(nx, ny)) {
                    grid_node.query_neighbors(player.x, player.y, view_range, player.id, &mut buf);
                    
                    if buf.len >= MAX_STREAM {
                        break 'search;
                    }
                }
            }
        }

        buf
    }
}


/// =====================================================
/// จำลองและวัดผลลัพธ์
/// =====================================================
fn simulate_zero_allocation_architecture(player_count: usize) -> f64 {
    let mut players: Vec<PlayerData> = (0..player_count)
        .map(|i| PlayerData {
            id: i as u32,
            x: (i as f32 * 13.0) % MAP_SIZE,
            y: (i as f32 * 17.0) % MAP_SIZE,
        })
        .collect();

    let mut router = Router::new();
    let start_time = Instant::now();

    for _tick in 0..TARGET_TPS {
        router.flush();

        for p in &mut players {
            p.x = (p.x + 1.0) % MAP_SIZE;
            p.y = (p.y + 1.0) % MAP_SIZE;
            
            router.update_player_location(*p);
        }

        let _all_streams: Vec<StreamBuffer> = players
            .par_iter()
            .map(|p| router.stream_data_for(p))
            .collect();
    }

    start_time.elapsed().as_secs_f64()
}

fn main() {
    println!("=== ระบบประเมินขีดจำกัด (2 Layer Architecture + Zero Heap Allocation) ===");
    println!("- MAP SIZE: {}x{} | Grid Node รับผิดชอบพื้นที่ {}x{}", MAP_SIZE, MAP_SIZE, CELL_SIZE, CELL_SIZE);
    println!("- 1 Player สแกนกรอบ 4 เหลี่ยม (Grid): {}x{}", VIEW_RADIUS*2.0, VIEW_RADIUS*2.0);
    println!("- Router สูบและรวบรวมข้อมูลส่งกลับสูงสุด (Stream): {} คน", MAX_STREAM);
    println!("- Server ความเร็ว: {} TPS (Ticks per second)\n", TARGET_TPS);

    println!("กำลังเริ่มการทดสอบแบบกรอบ 4 เหลี่ยม Bounding Box (แทนวงกลม)...\n");

    let mut current_ccu = 50_000;
    
    loop {
        let elapsed_time = simulate_zero_allocation_architecture(current_ccu);
        let status = if elapsed_time <= 1.0 { "✅ ไหวโคตรๆ" } else { "❌ สุดขีดจำกัด (เกิน 1 วิ)" };

        println!(
            "ทดสอบที่ {} Players | ลบ/สร้าง Data Object -> {} ครั้ง | เวลาผ่านไป: {:.3} วินาที | {}",
            current_ccu,
            current_ccu * TARGET_TPS,
            elapsed_time,
            status
        );

        if elapsed_time > 1.0 {
            println!("\n🚀 สรุป: ทดสอบรันสูงสุดจบที่: {} Players! (CCU)", current_ccu);
            break;
        }

        current_ccu += 50_000;
    }
}
