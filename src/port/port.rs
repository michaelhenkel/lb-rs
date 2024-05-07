use std::net::Ipv4Addr;
use async_timer::Oneshot;
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
        let time_per_packet_nanos = (time_per_packet_micros * 1000.0)/100.0;
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
                                for i in 0..packets{
                                    oif_idx = if i % flowlet_size == 0{
                                        async_timer::oneshot::Timer::new(std::time::Duration::from_micros(16)).await;
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
                                    if let Err(e) = self.egress_ports[oif_idx].data_channel().send(()).await{
                                        println!("Error sending to port: {}", e);
                                    }
                                    self.state.tx_packets += packets;
                                    self.state.tx_bytes += self.mtu as u64;
                                    async_timer::oneshot::Timer::new(tokio::time::Duration::from_nanos(time_per_packet_nanos as u64)).await;
                                }
                            },
                            LoadBalancerMode::StaticHash => {
                                let oif_idx = (flow.hash() % num_of_output_ports as u64) as usize;
                                self.egress_ports[oif_idx].inc_flows().await;
                                for _i in 0..packets{
                                    if let Err(e) = self.egress_ports[oif_idx].data_channel().send(()).await{
                                        println!("Error sending to port: {}", e);
                                    }
                                    self.state.tx_packets += packets;
                                    self.state.tx_bytes += self.mtu as u64;
                                    async_timer::oneshot::Timer::new(tokio::time::Duration::from_nanos(time_per_packet_nanos as u64)).await;
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
    port_name: String,
    command_rx: tokio::sync::mpsc::Receiver<EgressPortCommand>,
    state: PortState,
    client: EgressPortClient,
    mtu: u16,
    bps: u64,
    data_rx: tokio::sync::mpsc::Receiver<()>,
    buffer_size: u16,
}

impl EgressPort{
    pub fn new(port_name: String, bps: u64, mtu: u16, buffer_size: u16) -> EgressPort{
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(1);
        let (data_tx, data_rx) = tokio::sync::mpsc::channel(buffer_size as usize);
        EgressPort{
            port_name,
            command_rx,
            state: PortState::default(),
            client: EgressPortClient{command_tx, data_tx},
            mtu,
            bps,
            data_rx,
            buffer_size,
        }
    }
    pub fn client(&self) -> EgressPortClient{
        self.client.clone()
    }
    pub async fn run(mut self) -> anyhow::Result<()>{
        let byte_per_sec = self.bps;
        let mtu = self.mtu;
        let pps = byte_per_sec / mtu as u64;
        let time_per_packet = 1.0 / pps as f64;
        let time_per_packet_millis = time_per_packet * 1000.0;
        let time_per_packet_micros = time_per_packet_millis * 1000.0;
        let time_per_packet_nanos = (time_per_packet_micros * 1000.0)/100.0;
        loop{
            tokio::select! {
                command = self.command_rx.recv() => {
                    match command{
                        Some(EgressPortCommand::IncFlows()) => {
                            self.state.active_flows += 1;
                            self.state.total_flows += 1;
                        },
                        Some(EgressPortCommand::DecFlows()) => {
                            self.state.active_flows -= 1;
                        },
                        Some(EgressPortCommand::Stats(resp_tx)) => {
                            println!("Port: {}, Total Flows: {}, Active Flows: {}, rx_bytes: {}, tx_bytes: {}, rx_packet: {}, tx_packets: {}, dropped packets: {}", self.port_name, self.state.total_flows, self.state.active_flows, self.state.rx_bytes, self.state.tx_bytes, self.state.rx_packets, self.state.tx_packets, self.state.dropped_packets);
                            resp_tx.send(self.state.clone()).unwrap();
                        },
                        None => {}
                    }
                },
                _ = self.data_rx.recv() => {
                    if self.data_rx.len() == (self.buffer_size - 1) as usize{
                        self.state.dropped_packets += 1;
                    } else {
                        self.state.rx_bytes += self.mtu as u64;
                        self.state.rx_packets += 1;
                    }
                    async_timer::oneshot::Timer::new(tokio::time::Duration::from_nanos(time_per_packet_nanos as u64)).await;
                } 
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
    command_tx: tokio::sync::mpsc::Sender<EgressPortCommand>,
    data_tx: tokio::sync::mpsc::Sender<()>,
}

impl EgressPortClient{
    pub async fn inc_flows(&self){
        self.command_tx.send(EgressPortCommand::IncFlows()).await.unwrap();
    }
    pub async fn dec_flows(&self){
        self.command_tx.send(EgressPortCommand::DecFlows()).await.unwrap();
    }
    pub async fn stats(&self) -> anyhow::Result<PortState>{
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx.send(EgressPortCommand::Stats(tx)).await.unwrap();
        match rx.await{
            Ok(state) => return Ok(state),
            Err(e) => return Err(e.into()),
        }
    }
    pub fn data_channel(&self) -> tokio::sync::mpsc::Sender<()>{
        self.data_tx.clone()
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
    pub total_flows: u32,
    pub active_flows: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub dropped_packets: u64,
}