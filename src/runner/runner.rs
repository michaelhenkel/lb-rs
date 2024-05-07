use std::process::exit;
use crate::port::port::{EgressPortClient, IngressPortClient};

pub struct Runner{
    ingress_port_client_list: Vec<IngressPortClient>,
    egress_port_client_list: Vec<EgressPortClient>,
}

impl Runner{
    pub fn new(ingress_port_client_list: Vec<IngressPortClient>, egress_port_client_list: Vec<EgressPortClient>) -> Runner{
        Runner{
            ingress_port_client_list,
            egress_port_client_list,
        }
    }
    pub async fn run(self){
        let mut jh_list = Vec::new();
        let now = tokio::time::Instant::now();
        for ingress_port_client in self.ingress_port_client_list{
            let jh: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move{
                let _flows =  ingress_port_client.send_flows().await;
                return Ok(());
            });
            jh_list.push(jh);
        }
        futures::future::join_all(jh_list).await;
        let elapsed = now.elapsed().as_secs_f64();
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let mut total_rx_bytes = 0;
        let mut total_rx_packets = 0;
        for egress_port in &self.egress_port_client_list{
            let stats = egress_port.stats().await.unwrap();
            total_rx_bytes += stats.rx_bytes;
            total_rx_packets += stats.rx_packets;
        }
        let bytes_per_second = total_rx_bytes as f64 / elapsed;
        let bytes_per_second = byte_unit::Byte::from_f64_with_unit(bytes_per_second, byte_unit::Unit::B).unwrap().get_adjusted_unit(byte_unit::Unit::Gbit);
        let bytes_per_second: String = format!("{bytes_per_second:#.2}");

        let total_bytes = byte_unit::Byte::from_u64_with_unit(total_rx_bytes, byte_unit::Unit::B).unwrap().get_adjusted_unit(byte_unit::Unit::MB);
        let total_bytes: String = format!("{total_bytes:#.0}");
        println!("Total time: {}, total packets {}, total bytes {}, per_sec {}ps", elapsed, total_rx_packets, total_bytes.to_string(), bytes_per_second);
        println!("All flows sent");
        exit(0)
    }
}