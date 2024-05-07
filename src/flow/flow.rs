use std::{collections::hash_map::DefaultHasher, hash::{Hash, Hasher}};

#[derive(Clone)]
pub struct Flow{
    pub src_ip: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub dst_ip: u32,
    pub volume: u64,
    pub bps: u64,
}

impl Flow{
    pub fn new(src_ip: u32, src_port: u16, dst_port: u16, dst_ip: u32, volume: u64, bps: u64) -> Flow{
        Flow{
            src_ip,
            src_port,
            dst_ip,
            dst_port,
            volume,
            bps,
        }
    }
    pub fn start(&self) -> f64 {
        //println!("Starting flow with src_port: {}, dst_port: {}, dst_ip: {}, volume: {}, bps: {}", self.src_port, self.dst_port, self.dst_ip, self.volume, self.bps);
        let volume = self.volume;
        let bps = self.bps;
        let number_of_seconds = volume as f64 / bps as f64;
        number_of_seconds
    }
    pub fn hash(&self) -> u64{
        let mut hasher = DefaultHasher::new();
        self.src_ip.hash(&mut hasher);
        self.dst_ip.hash(&mut hasher);
        self.src_port.hash(&mut hasher);
        self.dst_port.hash(&mut hasher);
        hasher.finish()
    }
}