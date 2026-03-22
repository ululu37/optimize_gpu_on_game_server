# 🚀 Pure GPU Spatial Hashing Engine

ระบบ Game Server ประสิทธิภาพสูง พัฒนาด้วยภาษา Rust ที่ออกแบบมาเพื่อรองรับผู้เล่นพร้อมกันได้สูงสุดถึง **1,500,000 Player** ที่ความเร็วคงที่ **30 Ticks Per Second (TPS)** โดยเน้นการประมวลผลบน GPU ทั้งหมดเพื่อหาตำแหน่งและการมองเห็นของผู้เล่น (Spatial Partitioning & Neighbor Discovery)

## 📊 ผลการทดสอบการเพิ่มประสิทธิภาพ (Optimization Journey)

| วิธีการ (Methodology) | ความจุ (Players) | การใช้งาน GPU | ความเสถียร / ข้อจำกัด (Bottlenecks) |
| :--- | :--- | :--- | :--- |
| **ขั้นที่ 1**: CPU (16 Core) + Simple Array Loop | 1,000 | 0% | ประสิทธิภาพลดลงแบบ Exponential (O(N²)) |
| **ขั้นที่ 2**: CPU (16 Core) + Hash Grid (9 cells) | 90,000 | 0% | ติดคอขวดที่ความเร็วการเข้าถึง Memory ของ CPU |
| **ขั้นที่ 3**: Hybrid CPU-GPU (CPU Grid, GPU Stream) | 500,000 | 30% | ติดคอขวดที่การส่งข้อมูล (Sync CPU ↔ GPU) |
| **ขั้นที่ 4**: Full GPU + โครงสร้าง Array จองพื้นที่เผื่อ | 500,000 | 100% | กราฟนิ่ง แต่ GPU ทำงานหนักเต็มที่ |
| **ขั้นที่ 5**: Full GPU + Linked List (Optimized Mem) | 900,000 | 70% | **PCIe Bandwidth Limited**: ส่งข้อมูลผ่าน PCIe ไม่ทัน |
| **ขั้นที่ 6**: Full GPU + Linked List (เพิ่ม Padding เยอะ) | 500,000 | 100% | GPU ทำงานหนักเกินไปเพื่อรักษาความเสถียร |
| **ขั้นที่ 7**: Full GPU + Linked List (Onboard GPU) | 900,000 | 70% | **Unified Memory**: ตัดปัญหา PCIe ทำให้กราฟ GPU นิ่ง |
| **ขั้นที่ 8**: Full GPU + L2 Cache Optimized Grid | **1,200,000+** | **70%** | **ประสิทธิภาพสูงสุด (ลุ้นได้ถึง 1.5M)** |

## 🏗️ สถาปัตยกรรมทางเทคนิค (Technical Architecture)

โปรเจคนี้เลือกใช้ **Pure GPU Compute Shaders (WGSL)** ในการจัดการ Logic ของเกมทุกขั้นตอน เพื่อหลีกเลี่ยงการ Copy ข้อมูลระหว่าง CPU และ GPU ที่มีต้นทุนสูง

### แผนผังการทำงาน (Spatial Workflow)
```mermaid
graph TD
    A[Reset Pass: ล้างข้อมูล Hash Buckets] --> B[Movement Pass: คำนวณตำแหน่ง & สร้าง Link List]
    B --> C[Spatial Hashing: ใช้ atomicExchange จัดการ Head Node]
    C --> D[Neighbor Stream: ค้นหาผู้เล่นในระยะการมองเห็นแบบขนาน]
    D --> E[UDP Dispatch: ส่งข้อมูลด้วย Packet Buffers]
```

## 🛠️ จุดเด่นของการปรับแต่ง (Key Optimizations)

### 1. Unified Memory vs. PCIe
การรับส่งข้อมูลของผู้เล่นหลักล้านคน (ประมาณ 16MB - 32MB ต่อ Tick) ผ่าน PCIe มักจะเกิดอาการ "GPU รองาน" แต่เมื่อเปลี่ยนมาใช้ **Onboard GPU (Unified Memory)** จะช่วยลด Latency ในการส่งข้อมูล ทำให้ GPU สามารถประมวลผลคน 1.2M ได้อย่างไหลลื่นที่ Load เพียง 70%

### 2. การจัดการ L2 Cache
การจองพื้นที่ Grid แบบ `(player_count * 16 + grid_count * 8)` bytes ถูกคำนวณมาเพื่อให้ข้อมูลสำคัญถูกยกขึ้นไปอยู่บน **L2 Cache** ของ GPU ได้อย่างพอดี ช่วยให้การ `atomicExchange` และ `atomicAdd` ทำงานได้รวดเร็วที่สุด

### 3. โครงสร้าง Grid แบบ Link List
เลือกใช้โครงสร้างข้อมูลแบบ Linked List ภายใน GPU เพื่อประหยัดพื้นที่และรักษาตำแหน่งของข้อมูล (Memory Locality) ให้ข้อมูลที่อยู่ใกล้กันในเกม อยู่ใกล้กันในหน่วยความจำ

## 🚀 วิธีการรันโปรเจค

### ความต้องการของระบบ
- **Rust Compiler** (แนะนำรุ่นล่าสุด)
- **GPU ที่รองรับ WGPU** (Vulkan, DX12 หรือ Metal)

### ฝั่ง Server
```bash
cargo run --bin gpu
```

### ฝั่ง Viewer (Client Simulator)
```bash
cargo run --bin viewer
```

---
*โปรเจคนี้เป็นการวิจัยเชิงลึกเรื่องโครงสร้างพื้นฐานสำหรับเกมออนไลน์ขนาดหยักษ์ (Massive Scale) รับรองความเสถียรที่ 1.2M คน และ Peak ได้สูงสุด 1.5M คน ที่ 30 TPS*
