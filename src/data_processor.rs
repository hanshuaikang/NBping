use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use crate::ping_event::PingEvent;
use crate::ip_data::IpData;

pub struct DataProcessor {
    data_map: HashMap<String, IpData>, // key: addr_ip
    point_num: usize,
}

impl DataProcessor {
    pub fn new(targets: &[(String, String)], view_type: &str) -> Self {
        let point_num = if view_type == "point" || view_type == "sparkline" {
            200
        } else {
            10
        };
        let mut data_map = HashMap::new();
        
        for (addr, ip) in targets {
            let key = format!("{}_{}", addr, ip);
            data_map.insert(key, IpData {
                addr: addr.clone(),
                ip: ip.clone(),
                rtts: VecDeque::new(),
                last_attr: 0.0,
                min_rtt: 0.0,
                max_rtt: 0.0,
                timeout: 0,
                received: 0,
                pop_count: 0,
            });
        }
        
        Self { data_map, point_num }
    }
    
    pub fn process_event(&mut self, event: PingEvent) -> Option<IpData> {
        match event {
            PingEvent::Success { addr, ip, rtt, .. } => {
                let key = format!("{}_{}", addr, ip);
                if let Some(data) = self.data_map.get_mut(&key) {
                    Self::update_success_stats(data, rtt, self.point_num);
                    Some(data.clone())
                } else {
                    None
                }
            },
            PingEvent::Timeout { addr, ip, .. } => {
                let key = format!("{}_{}", addr, ip);
                if let Some(data) = self.data_map.get_mut(&key) {
                    Self::update_timeout_stats(data, self.point_num);
                    Some(data.clone())
                } else {
                    None
                }
            },
        }
    }
    
    fn update_success_stats(data: &mut IpData, rtt: f64, point_num: usize) {
        data.received += 1;
        data.last_attr = rtt;
        data.rtts.push_back(rtt);
        
        if data.min_rtt == 0.0 || rtt < data.min_rtt {
            data.min_rtt = rtt;
        }
        if rtt > data.max_rtt {
            data.max_rtt = rtt;
        }
        
        if data.rtts.len() > point_num {
            data.rtts.pop_front();
            data.pop_count += 1;
        }
    }
    
    fn update_timeout_stats(data: &mut IpData, point_num: usize) {
        data.rtts.push_back(-1.0);
        data.last_attr = -1.0;
        data.timeout += 1;
        
        if data.rtts.len() > point_num {
            data.rtts.pop_front();
            data.pop_count += 1;
        }
    }
    
}

pub fn start_data_processor(
    ping_event_rx: mpsc::Receiver<PingEvent>,
    ui_data_tx: mpsc::SyncSender<IpData>,
    targets: Vec<(String, String)>,
    view_type: String,
    running: Arc<Mutex<bool>>,
) {
    std::thread::spawn(move || {
        let mut processor = DataProcessor::new(&targets, &view_type);
        
        while *running.lock().unwrap() {
            match ping_event_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(event) => {
                    if let Some(updated_data) = processor.process_event(event) {
                        if ui_data_tx.send(updated_data).is_err() {
                            // UI channel closed, exit
                            break;
                        }
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Continue checking running flag
                    continue;
                },
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Network tasks finished
                    break;
                }
            }
        }
    });
}
