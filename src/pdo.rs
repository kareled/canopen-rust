use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Debug;

use embedded_can::{Frame, StandardId};
use embedded_can::nb::Can;
use hashbrown::HashMap;

use crate::error::CanAbortCode;
use crate::info;
use crate::node::{Node, NodeEvent};
use crate::object_directory::{ObjectDirectory, Variable};

#[derive(Debug, Clone, Copy)]
pub enum PdoType {
    TPDO,
    RPDO,
}

#[derive(Debug, Clone)]
pub struct PdoObject {
    pub pdo_type: PdoType,

    // Properties
    pub is_pdo_valid: bool,
    pub _not_used_rtr_allowed: bool,
    pub _not_used_is_29bit_can_id: bool,  // to differentiate CAN2.0A / CAN2.0B

    // Communication relativeX
    pub largest_sub_index: u8,
    pub cob_id: u16,
    pub transmission_type: u8,
    pub inhibit_time: u16,
    pub event_timer: u16,

    // Mapping relative
    pub num_of_map_objs: u8,
    pub mappings: Vec<(u16, u8, u8)>,  // index, sub_index, length
}

pub struct PdoObjects {
    pub rpdos: [PdoObject; 4],
    pub tpdos: [PdoObject; 4],
    pub cob_to_index: HashMap<u32, usize>,
}

impl PdoObject {
    pub fn update_comm_params(&mut self, var: &Variable) {
        if var.sub_index == 1 {
            let t: u32 = var.default_value.to();
            self.is_pdo_valid = (t >> 31 & 0x1) == 0;
            self._not_used_rtr_allowed = (t >> 30 & 0x1) == 1;
            self._not_used_is_29bit_can_id = (t >> 29 & 0x1) == 1;
            self.cob_id = (t & 0xFFFF) as u16;
        }
        match var.sub_index {
            0 => self.largest_sub_index = var.default_value.to(),
            2 => self.transmission_type = var.default_value.to(),
            3 => self.inhibit_time = var.default_value.to(),
            5 => self.event_timer = var.default_value.to(),
            _ => {}
        }
        // if var.index == 0x1801 {
        //     info!("xfguo: update_comm_params() 9, var = {:#x?}, pdo = {:#x?}", var, self);
        // }
    }

    pub fn update_map_params(&mut self, var: &Variable) {
        // info!("xfguo: update_map_params() 0. var = {:#x?}", var);
        if var.sub_index == 0 {
            let t = var.default_value.to();
            self.num_of_map_objs = t;
            self.mappings.resize((t + 1) as usize, (0, 0, 0));
            // if var.index == 0x1A01 {
            //     info!("xfguo: update_map_params() 1.1, var = {:#x?}, t = {}", var, t);
            // }
        } else if var.sub_index <= self.num_of_map_objs as u8 {
            let t: u32 = var.default_value.to();
            self.mappings[var.sub_index as usize] =
                ((t >> 16) as u16, ((t >> 8) & 0xFF) as u8, (t & 0xFF) as u8);
        }
        // if var.index == 0x1A01 {
        //     info!("xfguo: update_map_params() 1.1, pdo = {:#x?}", self);
        // }
    }
}

impl PdoObjects {
    pub fn new(od: &mut ObjectDirectory) -> Self {
        let default_rpdo = PdoObject {
            pdo_type: PdoType::RPDO,
            is_pdo_valid: false,
            _not_used_rtr_allowed: false,
            _not_used_is_29bit_can_id: false,
            largest_sub_index: 5,
            cob_id: 0x202,
            transmission_type: 0x01,
            inhibit_time: 0,
            event_timer: 0,
            num_of_map_objs: 0,
            mappings: vec![],
        };
        let default_tpdo = PdoObject {
            pdo_type: PdoType::TPDO,
            is_pdo_valid: false,
            _not_used_rtr_allowed: false,
            _not_used_is_29bit_can_id: false,
            largest_sub_index: 5,
            cob_id: 0x180,
            transmission_type: 0x01,
            inhibit_time: 0,
            event_timer: 0,
            num_of_map_objs: 0,
            mappings: vec![],
        };

        let rpdos = [(); 4].map(|_| default_rpdo.clone());
        let tpdos = [(); 4].map(|_| default_tpdo.clone());
        let mut res = PdoObjects { rpdos, tpdos, cob_to_index: HashMap::new() };
        for i in (0x1400..0x1C00).step_by(0x200) {
            for j in 0..4 {
                let idx = i + j;
                match od.get_variable(idx, 0) {
                    Ok(var) => {
                        res.update(var);
                        let len: u8 = var.default_value.to();
                        for k in 1..=len {
                            match od.get_variable(idx, k as u8) {
                                Ok(var) => res.update(var),
                                _ => {}  // ignore
                            }
                        }
                    }
                    _ => {}  // ignore
                }
            }
        }

        res
    }

    pub fn update(&mut self, var: &Variable) {
        let idx = var.index;
        let (t, x) = (idx >> 8, (idx & 0xF) as usize);
        match t {
            0x14 => self.rpdos[x].update_comm_params(var),
            0x16 => self.rpdos[x].update_map_params(var),
            0x18 => self.tpdos[x].update_comm_params(var),
            0x1A => self.tpdos[x].update_map_params(var),
            _ => {}
        }
    }
}

fn should_trigger_pdo(is_sync: bool, event: NodeEvent, transmission_type: u32, event_times: u32, count: u32) -> bool {
    if is_sync {
        if transmission_type == 0 || transmission_type > 240 || count % transmission_type != 0 {
            info!("xfguo: should_trigger_pdo 1.1.1, count = {}, transmission_type = {}", count, transmission_type);
            return false;
        }
    } else {
        match event {
            NodeEvent::NodeStart => { return true; }
            _ => {}
        }
        if transmission_type != 0xFE && transmission_type != 0xFF {
            // info!("xfguo: transmit_pdo_messages 1.1.2, i = {}, count = {}, tt = {}", i, count, tt);
            return false;
        } else if event_times == 0 || count % event_times != 0 {
            // info!("xfguo: transmit_pdo_messages 1.1.3, i = {}, count = {}, event_timer = {}", i, count, pdo.event_timer);
            return false;
        }
    }
    true
}

impl<CAN: Can> Node<CAN> where CAN::Frame: Frame + Debug {
    // TODO(zephyr): Change type to Sync / Event.
    pub(crate) fn transmit_pdo_messages(&mut self, is_sync: bool, event: NodeEvent, count: u32) {
        // info!("xfguo: transmit_pdo_messages 0");
        for i in 0..4 {
            let pdo = &self.pdo_objects.tpdos[i];

            if !pdo.is_pdo_valid {
                continue;
            }

            // Skip if don't need to transmit a PDO msg.
            // info!("xfguo: transmit_pdo_messages 1.1, pdo[{}] = {:#x?}", i, pdo);
            let tt = pdo.transmission_type as u32;
            if !should_trigger_pdo(is_sync, event, tt, pdo.event_timer as u32, count) {
                continue
            }

            // info!("xfguo: transmit_pdo_messages 2, pdo[{}] = {:#x?}", i, pdo);
            // Emit a TPDO message.
            match self.gen_pdo_frame(pdo.cob_id as u16, pdo.num_of_map_objs, pdo.mappings.clone()) {
                Ok(f) => {
                    info!("xfguo: try to send tpdo packet: {:?}", f);
                    match self.can_network.transmit(&f) {
                        Err(err) => {
                            info!("Errors in transmit TPDO frame, err: {:?}", err);
                        }
                        _ => {
                            info!("xfguo: sent tpdo packet: {:?}", f);
                        }
                    }
                }
                Err(err) => {
                    info!("Errors in generating PDO frame. err: {:?}", err);
                }
            }
        }
    }

    pub(crate) fn gen_pdo_frame(&mut self, cob_id: u16, num_of_map_objs: u8, mappings: Vec<(u16, u8, u8)>)
                                -> Result<CAN::Frame, CanAbortCode> {
        let mut t = Vec::new();
        // info!("xfguo: gen_pdo_frame() 0, {}, {:#x?}", num_of_map_objs, mappings);
        for i in 1..num_of_map_objs + 1 {
            let (idx, sub_idx, bits) = mappings[i as usize];
            match self.object_directory.get_variable(idx, sub_idx) {
                Ok(v) => {
                    t.push((vec_to_u64(&v.default_value.data), bits));
                    // info!("xfguo: gen_pdo_frame() 2.2.1, {:#x?}:{:#x?} => {:#x?}, t = {:#x?}",
                    //     idx, sub_idx, v, t);
                }
                Err(_) => return Err(CanAbortCode::GeneralError),
            }
        }
        let packet = pack_data(&t);
        Ok(CAN::Frame::new(StandardId::new(cob_id).unwrap(), packet.as_slice()).unwrap())
    }
}

fn vec_to_u64(v: &Vec<u8>) -> u64 {
    let mut res = 0u64;
    for x in v {
        res = (res << 8) | (*x as u64);
    }
    res
}

fn pack_data(vec: &Vec<(u64, u8)>) -> Vec<u8> {
    let mut merged = 0u64;
    let mut total_bits = 0u8;
    for (data, bits) in vec {
        total_bits += bits;
        // TODO(zephyr): optimize the expr below
        merged = (merged << bits) | (data & ((1 << bits) - 1));
    }
    let total_bytes = total_bits / 8 + if total_bits % 8 > 0 { 1 } else { 0 };
    let mut res = vec![0u8; total_bytes as usize];
    for i in 0..total_bytes {
        res[(total_bytes - 1 - i) as usize] = (merged & 0xFF) as u8;
        merged = merged >> 8;
    }
    res
}

fn unpack_data(vec: &Vec<u8>, bits: &Vec<u8>) -> Vec<(u64, u8)> {
    let mut data = vec_to_u64(vec);
    println!("{:#x}", data);
    let len = bits.len();
    let mut res = vec![(0u64, 0u8); len];
    for i in 0..len {
        let idx = len - 1 - i;
        let t = data & ((1 << bits[idx]) - 1);
        data = data >> bits[idx];
        res[idx] = (t, bits[idx]);
        println!("{:#x}, {:#x}, ", t, data);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut_data_with_bits(vec: &Vec<(u64, u8)>) -> Vec<(u64, u8)> {
        let mut res: Vec<(u64, u8)> = Vec::new();
        for (data, bits) in vec {
            res.push((data & ((1 << bits) - 1), *bits));
        }
        res
    }

    #[test]
    fn test_data_to_packet_and_packet_to_data() {
        // Convert initial_data to a packet
        let initial_data = vec![
            (0xABCDu64, 12),
            (0x123456u64, 20),
            (0x0102u64, 9),
        ];
        let packet = pack_data(&initial_data);
        println!("xfguo: packet = {:?}", packet);

        let result_data = unpack_data(&packet, &vec![12, 20, 9]);
        println!("xfguo: unpacked = {:?}", result_data);

        let cutted_data = cut_data_with_bits(&initial_data);
        assert_eq!(result_data, cutted_data);
    }
}