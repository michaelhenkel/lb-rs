use byte_unit::Byte;
use clap::Parser;
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};

use crate::port::port::LoadBalancerMode;

#[derive(Parser)]
pub struct Args{
    #[clap(short, long)]
    pub config_file: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Config{
    pub ingress_ports: Vec<InputPortConfig>,
    pub buffer_size: u32,
    pub egress_ports: u16,
    pub speed: String,
    pub mode: LoadBalancerMode,
    pub mtu: u16,
}

impl Config{
    pub fn new() -> anyhow::Result<Config>{
        let args = Args::parse();
        let config = std::fs::read_to_string(args.config_file)?;
        let mut config: Config = serde_yaml::from_str(&config)?;
        for input_port in &mut config.ingress_ports{
            if let Err(e) = Byte::parse_str(&config.speed, true){
                return Err(anyhow::anyhow!("Invalid speed: {}", e));
            }
            for flow in &mut input_port.flows{
                if let Err(e) = Byte::parse_str(&flow.volume, true){
                    return Err(anyhow::anyhow!("Invalid volume: {}", e));
                }
                if flow.src_port.is_none(){
                    let mut rng = thread_rng();
                    let src_port = rng.gen_range(40000..65535);
                    flow.src_port = Some(src_port);
                }
            }
        }
        Ok(config)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct InputPortConfig{
    pub flows: Vec<FlowConfig>,
    pub src_ip: Option<String>
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FlowConfig{
    pub volume: String,
    pub destination: String,
    pub src_port: Option<u16>,
}