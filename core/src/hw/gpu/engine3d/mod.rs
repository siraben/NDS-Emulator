use std::collections::VecDeque;

use crate::hw::mmu::IORegister;
use super::{GPU, Scheduler};

mod registers;
mod geometry;
mod rendering;

use geometry::*;
use registers::*;

pub struct Engine3D {
    cycles_ahead: i32,
    // Registers
    gxstat: GXSTAT,
    // Geometry Engine
    prev_command: GeometryCommand,
    params: Vec<u32>,
    gxfifo: VecDeque<GeometryCommandEntry>,
    gxpipe: VecDeque<GeometryCommandEntry>,
    // Matrices
    mtx_mode: MatrixMode,
    cur_proj: Matrix4,
    cur_pos: Matrix4,
    cur_vec: Matrix4,
    cur_tex: Matrix4,
    proj_stack_sp: u8,
    pos_vec_stack_sp: u8,
    tex_stack_sp: u8,
    proj_stack: [Matrix4; 1], // Projection Stack
    pos_stack: [Matrix4; 31], // Coordinate Stack
    vec_stack: [Matrix4; 31], // Directional Stack
    tex_stack: [Matrix4; 1], // Texture Stack
    // Rendering Engine
    viewport: Viewport,
    clear_color: ClearColor,
    clear_depth: ClearDepth,
    pixels: Vec<u16>,
    rendering: bool,
    // Polygons
    polygon_attrs: PolygonAttributes,
    // Textures
    tex_params: TextureParams,
}

impl Engine3D {
    const FIFO_LEN: usize = 256;
    const PIPE_LEN: usize = 4;

    pub fn new() -> Self {
        Engine3D {
            cycles_ahead: 0,
            // Registers
            gxstat: GXSTAT::new(),
            // Geometry Engine
            prev_command: GeometryCommand::Unimplemented,
            params: Vec::new(),
            gxfifo: VecDeque::with_capacity(256),
            gxpipe: VecDeque::with_capacity(4),
            // Matrices
            mtx_mode: MatrixMode::Proj,
            cur_proj: Matrix4::from_element(FixedPoint::zero()),
            cur_pos: Matrix4::from_element(FixedPoint::zero()),
            cur_vec: Matrix4::from_element(FixedPoint::zero()),
            cur_tex: Matrix4::from_element(FixedPoint::zero()),
            proj_stack_sp: 0,
            pos_vec_stack_sp: 0,
            tex_stack_sp: 0,
            proj_stack: [Matrix4::from_element(FixedPoint::zero()); 1], // Projection Stack
            pos_stack: [Matrix4::from_element(FixedPoint::zero()); 31], // Coordinate Stack
            vec_stack: [Matrix4::from_element(FixedPoint::zero()); 31], // Directional Stack
            tex_stack: [Matrix4::from_element(FixedPoint::zero()); 1], // Texture Stack
            // Rendering Engine
            viewport: Viewport::new(),
            clear_color: ClearColor::new(),
            clear_depth: ClearDepth::new(),
            pixels: vec![0; GPU::WIDTH * GPU::HEIGHT],
            rendering: false,
            // Polygons
            polygon_attrs: PolygonAttributes::new(),
            // Textures
            tex_params: TextureParams::new(),
        }
    }
    
    pub fn clock(&mut self, cycles: usize) {
        self.cycles_ahead += cycles as i32;
        while self.cycles_ahead > 0 {
            if let Some(command_entry) = self.gxpipe.pop_front() {
                self.exec_command(command_entry);
                while self.gxpipe.len() < 3 {
                    if let Some(command_entry) = self.gxfifo.pop_front() {
                        self.gxpipe.push_back(command_entry);
                    } else { break }
                }
            } else { break }
        }
    }
}


impl Engine3D {
    pub fn read_register(&self, addr: u32) -> u8 {
        assert_eq!(addr >> 12, 0x04000);
        match addr & 0xFFF {
            0x600 ..= 0x603 => self.read_gxstat((addr as usize) & 0x3),
            _ => { warn!("Ignoring Engine3D Read at 0x{:08X}", addr); 0 },
        }
    }

    pub fn write_register(&mut self, scheduler: &mut Scheduler, addr: u32, value: u8) {
        assert_eq!(addr >> 12, 0x04000);
        match addr & 0xFFF {
            0x350 ..= 0x353 => self.clear_color.write(scheduler, addr as usize & 0x3, value),
            0x354 ..= 0x355 => self.clear_depth.write(scheduler, addr as usize & 0x1, value),
            0x600 ..= 0x603 => self.write_gxstat(scheduler, (addr as usize) & 0x3, value),
            _ => warn!("Ignoring Engine3D Write 0x{:08X} = {:02X}", addr, value),
        }
    }
}