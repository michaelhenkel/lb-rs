use std::net::Ipv4Addr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use byte_unit::Byte;
use rand::Rng;
use serde::{Deserialize, Serialize};
use crate::config::config::FlowConfig;
use crate::flow::flow::Flow;


const DST_PORT: u16 = 80;


pub struct IngressPort{
    ip: u32,
    flows: Vec<FlowConfig>,
    port_name: String,
    command_rx: tokio::sync::mpsc::Receiver<IngressPortCommand>,
    state: PortState,
    client: IngressPortClient,
    lb_mode: LoadBalancerMode,
    mtu: u16,
    bps: u64,
    egress_ports: Vec<EgressPortClient>,
}

impl IngressPort{
    pub fn new(ip: u32, flows: Vec<FlowConfig>, port_name: String, bps: u64, lb_mode: LoadBalancerMode, mtu: u16, egress_ports: Vec<EgressPortClient>) -> IngressPort{
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        IngressPort{
            ip,
            flows,
            port_name,
            command_rx: rx,
            state: PortState::default(),
            client: IngressPortClient{command_tx: tx},
            lb_mode,
            mtu,
            bps,
            egress_ports,
        }
    }
    pub fn client(&self) -> IngressPortClient{
        self.client.clone()
    }
    pub async fn run(mut self) -> anyhow::Result<()>{
        let byte_per_sec = self.bps;
        let mtu = self.mtu;
        let pps = byte_per_sec / mtu as u64;
        let time_per_packet = 1.0 / pps as f64;
        let time_per_packet_millis = time_per_packet * 1000.0;
        let time_per_packet_micros = time_per_packet_millis * 1000.0;
        let time_per_packet_nanos = time_per_packet_micros * 1000.0;
        let mut interval = tokio::time::interval(tokio::time::Duration::from_nanos(time_per_packet_nanos as u64));
        while let Some(ingress_command) = self.command_rx.recv().await{
            match ingress_command{
                IngressPortCommand::SendFlows(resp_tx) => {
                    let num_of_output_ports = self.egress_ports.len();
                    for flow_config in &self.flows{
                        let dst_ip: u32 = u32::from_be_bytes(flow_config.destination.parse::<Ipv4Addr>().unwrap().octets());
                        let src_port = if let Some(src_port) = flow_config.src_port{
                            src_port
                        }else{
                            let mut rng = rand::thread_rng();
                            rng.gen_range(40000..65535)
                        };
                        let volume = Byte::parse_str(&flow_config.volume, true).unwrap().as_u64();
                        let flow = Flow::new(self.ip, src_port, DST_PORT, dst_ip, volume, byte_per_sec);
                        
                        let mut packets = volume / self.mtu as u64;
                        let remainder = volume % self.mtu as u64;
                        if remainder > 0{
                            packets += 1;
                        }
                        match self.lb_mode{
                            LoadBalancerMode::Dfls(flowlet_size) => {
                                let mut oif_idx = { 
                                    let mut rng = rand::thread_rng();
                                    rng.gen_range(0..num_of_output_ports)
                                };
                                let mut prev_port: Option<u16> = None;

                                while packets > 0{
                                    interval.tick().await;
                                    packets -= 1;
                                    oif_idx = if packets % flowlet_size == 0{
                                        oif_idx = (oif_idx + 1) % num_of_output_ports;
                                        self.egress_ports[oif_idx].inc_flows().await;
                                        if let Some(ref p) = prev_port{
                                            self.egress_ports[*p as usize].dec_flows().await;
                                        }
                                        prev_port = Some(oif_idx as u16);
                                        oif_idx
                                    } else {
                                        oif_idx
                                    };
                                    self.egress_ports[oif_idx].add_to_buffer();
                                    
                                }
                            },
                            LoadBalancerMode::StaticHash => {
                                let oif_idx = (flow.hash() % num_of_output_ports as u64) as usize;
                                self.egress_ports[oif_idx].inc_flows().await;
                                while packets > 0{
                                    interval.tick().await;
                                    packets -= 1;
                                    self.egress_ports[oif_idx].add_to_buffer();
                                }
                                self.egress_ports[oif_idx].dec_flows().await;
                            },
                        }
                    }
                    resp_tx.send(self.flows.len()).unwrap();

                },
                IngressPortCommand::Stats(resp_tx) => {
                    println!("Port: {}, Total Flows: {}, Active Flows: {}, rx_bytes: {}, tx_bytes: {}, rx_packet: {}, tx_packets: {}, dropped packets: {}", self.port_name, self.state.total_flows, self.state.active_flows, self.state.rx_bytes, self.state.tx_bytes, self.state.rx_packets, self.state.tx_packets, self.state.dropped_packets);
                    resp_tx.send(()).unwrap();
                }
            }
        }
        Ok(())
    }
}

pub enum IngressPortCommand{
    SendFlows(tokio::sync::oneshot::Sender<usize>),
    Stats(tokio::sync::oneshot::Sender<()>)
}

#[derive(Clone)]
pub struct IngressPortClient{
    command_tx: tokio::sync::mpsc::Sender<IngressPortCommand>,
}

impl IngressPortClient{
    pub async fn send_flows(&self) -> usize{
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx.send(IngressPortCommand::SendFlows(tx)).await.unwrap();
        let res = match rx.await{
            Ok(num) => num,
            Err(_) => panic!("Error sending flows"),
        };
        res
    }
    pub async fn stats(&self) -> anyhow::Result<()>{
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx.send(IngressPortCommand::Stats(tx)).await.unwrap();
        if let Err(e) = rx.await{
            println!("Error getting stats: {}", e);
        }
        Ok(())
    }

}


pub struct EgressPort{
    client: EgressPortClient,
    mtu: u16,
    bps: u64,
    buffer: Arc<AtomicU64>,
    rx_packets: Arc<AtomicU64>,
}

impl EgressPort{
    pub fn new(port_name: String, bps: u64, mtu: u16, buffer_size: u32) -> EgressPort{
        let buffer = Arc::new(AtomicU64::new(0));
        let rx_packets = Arc::new(AtomicU64::new(0));
        let dropped_packets = Arc::new(AtomicU64::new(0));
        let active_flows = Arc::new(AtomicU64::new(0));
        let total_flows = Arc::new(AtomicU64::new(0));

        EgressPort{
            client: EgressPortClient{
                port_name: port_name.clone(),
                buffer: buffer.clone(),
                rx_packets: rx_packets.clone(),
                dropped_packets,
                active_flows,
                total_flows,
                buffer_size,
                mtu,
            },
            mtu,
            bps,
            buffer,
            rx_packets,
        }
    }
    pub fn client(&self) -> EgressPortClient{
        self.client.clone()
    }
    pub async fn run(self) -> anyhow::Result<()>{
        let byte_per_sec = self.bps;
        let mtu = self.mtu;
        let pps = byte_per_sec / mtu as u64;
        let time_per_packet = 1.0 / pps as f64;
        let time_per_packet_millis = time_per_packet * 1000.0;
        let time_per_packet_micros = time_per_packet_millis * 1000.0;
        let time_per_packet_nanos = time_per_packet_micros * 1000.0;
        let mut interval = tokio::time::interval(tokio::time::Duration::from_nanos(time_per_packet_nanos as u64));

        loop{
            interval.tick().await;
            if self.buffer.load(std::sync::atomic::Ordering::Relaxed) > 0{
                self.buffer.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                self.rx_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    
}

pub enum EgressPortCommand{
    IncFlows(),
    DecFlows(),
    Stats(tokio::sync::oneshot::Sender<PortState>),
}

#[derive(Clone)]
pub struct EgressPortClient{
    port_name: String,
    buffer: Arc<AtomicU64>,
    rx_packets: Arc<AtomicU64>,
    dropped_packets: Arc<AtomicU64>,
    active_flows: Arc<AtomicU64>,
    total_flows: Arc<AtomicU64>,
    buffer_size: u32,
    mtu: u16,
}

impl EgressPortClient{
    pub async fn inc_flows(&self){
        self.active_flows.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.total_flows.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    pub async fn dec_flows(&self){
        self.active_flows.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
    pub async fn stats(&self) -> anyhow::Result<PortState>{
        let port_stats = PortState{
            port_name: self.port_name.clone(),
            total_flows: self.total_flows.load(std::sync::atomic::Ordering::Relaxed) as u32,
            active_flows: self.active_flows.load(std::sync::atomic::Ordering::Relaxed) as u32,
            rx_bytes: self.rx_packets.load(std::sync::atomic::Ordering::Relaxed) * self.mtu as u64,
            tx_bytes: 0,
            tx_packets: 0,
            rx_packets: self.rx_packets.load(std::sync::atomic::Ordering::Relaxed),
            dropped_packets: self.dropped_packets.load(std::sync::atomic::Ordering::Relaxed),
        };
        Ok(port_stats)
    }
    pub fn add_to_buffer(&self){
        if self.buffer.load(std::sync::atomic::Ordering::Relaxed) == self.buffer_size as u64{
            self.dropped_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.buffer.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    pub fn buffer_size(&self) -> u64{
        self.buffer.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "variant", content = "flowlet_size")]
pub enum LoadBalancerMode{
    StaticHash,
    Dfls(u64),
}


#[derive(Default, Clone, Debug)]
pub struct PortState{
    pub port_name: String,
    pub total_flows: u32,
    pub active_flows: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub dropped_packets: u64,
}

impl std::fmt::Display for PortState{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result{
        write!(f, "Port: {}, Total Flows: {}, Active Flows: {}, rx_bytes: {}, tx_bytes: {}, rx_packet: {}, tx_packets: {}, dropped packets: {}", self.port_name, self.total_flows, self.active_flows, self.rx_bytes, self.tx_bytes, self.rx_packets, self.tx_packets, self.dropped_packets)
    }
}