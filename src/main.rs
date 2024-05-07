use std::net::Ipv4Addr;

use byte_unit::Byte;
use ipnet::Ipv4Net;
use port::port::{EgressPort, IngressPort};

use crate::{config::config::Config, runner::runner::Runner};

pub mod flow;
pub mod port;
pub mod config;
pub mod runner;

const IP_NETWORK: &str = "10.0.0.0/8";

#[tokio::main]
async fn main() -> anyhow::Result<()>{
    let config = Config::new()?;
    let ip_network: Ipv4Net = IP_NETWORK.parse()?; 
    let bps = Byte::parse_str(&config.speed, true)?.as_u64();

    let mut jh_list = Vec::new();

    let lb_mode = config.mode;
    let mut egress_port_client_list = Vec::new();
    for i in 0..config.egress_ports{
        let egress_port = EgressPort::new(format!("egress_port_{}", i), bps, config.mtu, config.buffer_size);
        egress_port_client_list.push(egress_port.client());
        let jh = tokio::spawn(async move{
            if let Err(e) = egress_port.run().await{
                eprintln!("Error running egress port: {}", e);
            }
        });
        jh_list.push(jh);
    }

    let mut ingress_port_client_list = Vec::new();
    for (idx, input_port) in config.ingress_ports.iter().enumerate(){
        let src_ip = if let Some(src_ip) = &input_port.src_ip{
            u32::from_be_bytes(src_ip.parse::<Ipv4Addr>().unwrap().octets())
        } else {
            let port_ip = ip_network.addr();
            u32::from_be_bytes(port_ip.octets()) + idx as u32 + 1
        };
        let ingress_port = IngressPort::new(src_ip, input_port.flows.clone(), format!("ingress_port_{}", idx), bps, lb_mode.clone(), config.mtu, egress_port_client_list.clone());
        ingress_port_client_list.push(ingress_port.client());
        let jh = tokio::spawn(async move{
            if let Err(e) = ingress_port.run().await{
                eprintln!("Error running ingress port: {}", e);
            }
        });
        jh_list.push(jh);
    }

    let runner = Runner::new(ingress_port_client_list.clone(), egress_port_client_list.clone());
    let jh = tokio::spawn(async move{
        runner.run().await;
    });
    jh_list.push(jh);

    futures::future::join_all(jh_list).await;

    Ok(())
    
}
