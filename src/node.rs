use embedded_can::{nb::Can, Frame, Id, StandardId};

use crate::object_directory::ObjectDirectory;
use crate::pdo::PdoObjects;
use crate::prelude::*;
use crate::sdo_server::SdoState;
use crate::sdo_server::SdoState::Normal;
use crate::util::get_cob_id;
use crate::info;

const DEFAULT_BLOCK_SIZE: u8 = 0x7F;

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum NodeState {
    Init,
    PreOperational,
    Operational,
    Stopped,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum NodeEvent {
    RegularTimerEvent = 1,
    NodeStart,
    Unused = 0xFF,
}

pub struct Node<CAN> where CAN: Can, CAN::Frame: Frame + Debug {
    pub(crate) node_id: u16,  // TODO(zephyr): should be u8 for "CANOPEN 2.0a"
    pub(crate) can_network: CAN,
    pub(crate) object_directory: ObjectDirectory,
    pub(crate) pdo_objects: PdoObjects,

    // SDO specific data below:
    pub(crate) sdo_state: SdoState,
    // TODO(zephyr): Let's use &Vec<u8> instead. 这个需要重点思考下。
    pub(crate) read_buf: Option<Vec<u8>>,
    pub(crate) read_buf_index: usize,
    pub(crate) next_read_toggle: u8,
    pub(crate) write_buf: Option<Vec<u8>>,
    pub(crate) reserved_index: u16,
    pub(crate) reserved_sub_index: u8,
    pub(crate) write_data_size: u32,
    pub(crate) need_crc: bool,
    pub(crate) block_size: u8,
    // sequences_per_block?
    pub(crate) current_seq_number: u8,
    pub(crate) crc_enabled: bool,

    pub(crate) sync_count: u32,
    pub(crate) event_count: u32,
    pub(crate) state: NodeState,
}

impl<CAN: Can> Node<CAN> where CAN::Frame: Frame + Debug {
    pub fn new(
        node_id: u16,
        eds_content: &str,
        can_network: CAN,
    ) -> Self {
        let mut object_directory = ObjectDirectory::new(node_id, eds_content);
        let pdo_objects = PdoObjects::new(&mut object_directory);
        Node {
            node_id,
            can_network,
            object_directory,
            pdo_objects,
            sdo_state: Normal,
            read_buf: None,
            read_buf_index: 0,
            write_buf: None,
            reserved_index: 0,
            reserved_sub_index: 0,
            write_data_size: 0,
            need_crc: false,
            block_size: DEFAULT_BLOCK_SIZE,
            current_seq_number: 0,
            next_read_toggle: 0,
            crc_enabled: true,
            sync_count: 0,
            event_count: 0,
            state: NodeState::Init,
        }
    }

    pub(crate) fn filter_frame(&self, frame: &CAN::Frame) -> bool {
        if let Some(cob_id) = get_cob_id(frame) {
            if cob_id & 0x7F == self.node_id {
                return false;
            }
        }
        true
    }

    fn reset_communication(&mut self) {
        todo!();
    }

    fn reset(&mut self) {
        // regular reset

        // reset communication
        self.reset_communication();

        todo!();
    }

    fn process_nmt_frame(&mut self, frame: &CAN::Frame) -> Option<CAN::Frame> {
        info!("xfguo: process_nmt_frame 0: {:?}", frame);
        if frame.dlc() != 2 {
            return None;
        }
        let (cs, nid) = (frame.data()[0], frame.data()[1]);
        info!("xfguo: process_nmt_frame 1: cs = {:#x}, nid = {}", cs, nid);
        if nid != self.node_id as u8 {
            return None;
        }
        match cs {
            1 => {
                info!("NMT: change state to OPERATIONAL");
                self.state = NodeState::Operational;
                self.trigger_event(NodeEvent::NodeStart);
            },
            2 => if self.state != NodeState::Init {
                info!("NMT: change state to STOPPED");
                self.state = NodeState::Stopped;
            },
            0x80 => {
                info!("NMT: change state to PRE-OPERATIONAL");
                self.state = NodeState::PreOperational
            },
            0x81 => {
                info!("NMT: change state to INIT, will reset the whole system");
                self.state = NodeState::Init;
                self.reset();
            },
            0x82 => {
                info!("NMT: change state to INIT, will reset the communication");
                self.state = NodeState::Init;
                self.reset_communication();
            },
            _ => {},
        }
        None
    }

    fn process_sync_frame(&mut self) -> Option<CAN::Frame> {
        if self.state == NodeState::Operational {
            self.sync_count += 1;
            self.transmit_pdo_messages(true, NodeEvent::Unused, self.sync_count);
        }
        None
    }

    pub fn trigger_event(&mut self, event: NodeEvent) {
        self.event_count = 0;
        self.sync_count = 0;
        self.transmit_pdo_messages(false, event, self.event_count);
    }

    pub fn event_timer_callback(&mut self) {
        // info!("xfguo: event_timer_callback 0");
        if self.state == NodeState::Operational {
            self.event_count += 1;
            self.transmit_pdo_messages(false, NodeEvent::RegularTimerEvent, self.event_count);
        }
    }

    pub fn communication_object_dispatch(&mut self, frame: CAN::Frame) -> Option<CAN::Frame> {
        let cob_id = get_cob_id(&frame).unwrap();
        match cob_id & 0xFF80 {
            0x000 => self.process_nmt_frame(&frame),
            0x080 => self.process_sync_frame(),
            0x600 => self.dispatch_sdo_request(&frame),
            _ => None,
        }
    }

    pub fn init(&mut self) {
        let ready_frame = Frame::new(StandardId::new(0x234).unwrap(), &[1, 2, 3, 5]).expect("");
        self.can_network
            .transmit(&ready_frame)
            .expect("Failed to send CAN frame");
    }
    //
    // fn transmit(&mut self, frame: &CAN::Frame, max_retries: i32) {
    //     for _ in 1..max_retries {
    //         match self.can_network.transmit(frame) {
    //             Ok(None) => return,
    //             Ok(Option::Some(f)) => self.transmit(&f, max_retries),
    //             Err(err) => {
    //                 info!("xfguo: Errors({:?}) in transmit frame, retry", err);
    //             }
    //         }
    //     }
    //     info!("xfguo: Failed to transmit frame {:?} after {:?} retries", frame, max_retries);
    // }

    // Need to be non-blocking.
    pub fn process_one_frame(&mut self) {
        let frame = match self.can_network.receive() {
            Ok(f) => f,
            Err(nb::Error::WouldBlock) => return,  // try next time
            Err(nb::Error::Other(err)) => {
                info!("Errors in reading CAN frame, {:?}", err);
                return
            }
        };
        info!("[node] got frame: {:?}", frame);

        if let Some(response) = self.communication_object_dispatch(frame) {
            if let Id::Standard(sid) = response.id() {
                if sid.as_raw() == 0 {
                    // Don't need to send any reply for empty frame.
                    return;
                }
            }
            // info!("[node] to send reply : {:?}", response);
            self.can_network
                .transmit(&response)
                .expect("Failed to send CAN frame");
            info!("[node] sent a frame : {:?}", response);
        }
    }
}
