use std::cmp::{PartialEq, Eq, Reverse};
use std::hash::Hash;

use priority_queue::PriorityQueue;

use super::{
    AccessType,
    dma::{DMAChannel, DMAOccasion},
    mmu::MemoryValue,
    interrupt_controller::{InterruptController, InterruptRequest},
    gpu::{DISPSTAT, DISPSTATFlags, POWCNT1},
    spu,
    HW
};

impl HW {
    pub fn handle_events(&mut self, arm7_cycles: usize) {
        self.scheduler.cycle += arm7_cycles;
        while let Some(event) = self.scheduler.get_next_event() {
            (event.handler)(self, event.event);
        }
    }

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::DMA(is_nds9, num) => {
                let channel = self.get_channel(is_nds9, num);
                // TODO: Clean Up
                if channel.cnt.transfer_32 {
                    if is_nds9 {
                        self.run_dma(true, num, &HW::arm9_get_access_time::<u32>,
                            &HW::arm9_read::<u32>, &HW::arm9_write::<u32>);
                    } else {
                        self.run_dma(false, num, &HW::arm7_get_access_time::<u32>,
                            &HW::arm7_read::<u32>, &HW::arm7_write::<u32>);
                    }
                } else {
                    if is_nds9 {
                        self.run_dma(true, num, &HW::arm9_get_access_time::<u16>,
                            &HW::arm9_read::<u16>, &HW::arm9_write::<u16>);
                    } else {
                        self.run_dma(false, num, &HW::arm7_get_access_time::<u16>,
                            &HW::arm7_read::<u16>, &HW::arm7_write::<u16>);
                    }
                }
            },
            Event::StartNextLine => {
                let (vcount, start_vblank) = self.gpu.start_next_line(&mut self.scheduler);
                if start_vblank {
                    self.handle_event(Event::VBlank);
                    self.check_dispstats(&mut |dispstat, interrupts|
                        if dispstat.contains(DISPSTATFlags::VBLANK_IRQ_ENABLE) {
                            interrupts.request |= InterruptRequest::VBLANK;
                        }
                    );
                }
                self.check_dispstats(&mut |dispstat, interrupts|
                    if dispstat.contains(DISPSTATFlags::VBLANK_IRQ_ENABLE) && vcount == dispstat.vcount_setting {
                        interrupts.request |= InterruptRequest::VCOUNTER_MATCH;
                    }
                );
            },
            Event::HBlank => {
                if self.gpu.start_hblank(&mut self.scheduler) { self.run_dmas(DMAOccasion::HBlank) }
                self.check_dispstats(&mut |dispstat, interrupts|
                    if dispstat.contains(DISPSTATFlags::HBLANK_IRQ_ENABLE) {
                        interrupts.request |= InterruptRequest::HBLANK;
                    }
                );
            },
            Event::VBlank => {
                self.run_dmas(DMAOccasion::VBlank);
                if self.gpu.powcnt1.contains(POWCNT1::ENABLE_3D_RENDERING) {
                    self.gpu.engine3d.render(&self.gpu.vram)
                }
            },
            Event::TimerOverflow(_, _) => self.on_timer_overflow(event),
            Event::ROMWordTransfered => {
                self.cartridge.update_word();
                self.run_dmas(DMAOccasion::DSCartridge);
            },
            Event::ROMBlockEnded(is_arm7) => if self.cartridge.end_block() {
                self.interrupts[(!is_arm7) as usize].request |= InterruptRequest::GAME_CARD_TRANSFER_COMPLETION;
            },
            Event::GenerateAudioSample => self.spu.generate_sample(&mut self.scheduler),
            Event::StepAudioChannel(channel_spec) => match channel_spec {
                // TODO: Figure out how to avoid code duplication
                // TODO: Use SPU FIFO
                spu::ChannelSpec::Base(num) => {
                    let format = self.spu.base_channels[num].format();
                    match format {
                        spu::Format::PCM8 => {
                            let (addr, reset) = self.spu.base_channels[num].next_addr_pcm::<u8>();
                            self.spu.base_channels[num].schedule(&mut self.scheduler, reset);
                            let sample = self.arm7_read::<u8>(addr);
                            self.spu.base_channels[num].set_sample(sample);
                        },
                        spu::Format::PCM16 => {
                            let (addr, reset) = self.spu.base_channels[num].next_addr_pcm::<u16>();
                            self.spu.base_channels[num].schedule(&mut self.scheduler, reset);
                            let sample = self.arm7_read::<u16>(addr);
                            self.spu.base_channels[num].set_sample(sample);
                        },
                        spu::Format::ADPCM => {
                            let reset = if let Some(addr) = self.spu.base_channels[num].initial_adpcm_addr() {
                                let value = self.arm7_read::<u32>(addr);
                                self.spu.base_channels[num].set_initial_adpcm(value);
                                false
                            } else {
                                let (addr, reset) = self.spu.base_channels[num].next_addr_adpcm();
                                let value = self.arm7_read(addr);
                                self.spu.base_channels[num].set_adpcm_data(value);
                                reset
                            };
                            self.spu.base_channels[num].schedule(&mut self.scheduler, reset);
                        },
                        _ => todo!(),
                    }
                    if let Some((addr, capture_i, use_pcm8)) = self.spu.capture_addr(num) {
                        if use_pcm8 {
                            let value = self.spu.capture_data(capture_i);
                            self.arm7_write::<u8>(addr, value);
                        } else {
                            let value = self.spu.capture_data(capture_i);
                            self.arm7_write::<u16>(addr, value);
                        }
                    }
                },
                spu::ChannelSpec::PSG(num) => {
                    let format = self.spu.psg_channels[num].format();
                    match format {
                        spu::Format::PCM8 => {
                            let (addr, reset) = self.spu.psg_channels[num].next_addr_pcm::<u8>();
                            self.spu.psg_channels[num].schedule(&mut self.scheduler, reset);
                            let sample = self.arm7_read::<u8>(addr);
                            self.spu.psg_channels[num].set_sample(sample);
                        },
                        spu::Format::PCM16 => {
                            let (addr, reset) = self.spu.psg_channels[num].next_addr_pcm::<u16>();
                            self.spu.psg_channels[num].schedule(&mut self.scheduler, reset);
                            let sample = self.arm7_read::<u16>(addr);
                            self.spu.psg_channels[num].set_sample(sample);
                        },
                        spu::Format::ADPCM => {
                            let reset = if let Some(addr) = self.spu.psg_channels[num].initial_adpcm_addr() {
                                let value = self.arm7_read::<u32>(addr);
                                self.spu.psg_channels[num].set_initial_adpcm(value);
                                false
                            } else {
                                let (addr, reset) = self.spu.psg_channels[num].next_addr_adpcm();
                                let value = self.arm7_read(addr);
                                self.spu.psg_channels[num].set_adpcm_data(value);
                                reset
                            };
                            self.spu.psg_channels[num].schedule(&mut self.scheduler, reset);
                        },
                        _ => todo!(),
                    }
                },
                spu::ChannelSpec::Noise(num) => {
                    let format = self.spu.noise_channels[num].format();
                    match format {
                        spu::Format::PCM8 => {
                            let (addr, reset) = self.spu.noise_channels[num].next_addr_pcm::<u8>();
                            self.spu.noise_channels[num].schedule(&mut self.scheduler, reset);
                            let sample = self.arm7_read::<u8>(addr);
                            self.spu.noise_channels[num].set_sample(sample);
                        },
                        spu::Format::PCM16 => {
                            let (addr, reset) = self.spu.noise_channels[num].next_addr_pcm::<u16>();
                            self.spu.noise_channels[num].schedule(&mut self.scheduler, reset);
                            let sample = self.arm7_read::<u16>(addr);
                            self.spu.noise_channels[num].set_sample(sample);
                        },
                        spu::Format::ADPCM => {
                            let reset = if let Some(addr) = self.spu.noise_channels[num].initial_adpcm_addr() {
                                let value = self.arm7_read::<u32>(addr);
                                self.spu.noise_channels[num].set_initial_adpcm(value);
                                false
                            } else {
                                let (addr, reset) = self.spu.noise_channels[num].next_addr_adpcm();
                                let value = self.arm7_read(addr);
                                self.spu.noise_channels[num].set_adpcm_data(value);
                                reset
                            };
                            self.spu.noise_channels[num].schedule(&mut self.scheduler, reset);
                        },
                        _ => todo!(),
                    }
                },
            },
            Event::ResetAudioChannel(channel_spec) => match channel_spec {
                spu::ChannelSpec::Base(num) => self.spu.base_channels[num].reset_sample(),
                spu::ChannelSpec::PSG(num) => self.spu.psg_channels[num].reset_sample(),
                spu::ChannelSpec::Noise(num) => self.spu.noise_channels[num].reset_sample(),
            },
        }
    }

    fn get_channel(&mut self, is_nds9: bool, num: usize) -> &mut DMAChannel {
        if is_nds9 { &mut self.dma9.channels[num] } else { &mut self.dma7.channels[num] }
    }

    fn run_dma<A, R, W, T: MemoryValue>(&mut self, is_nds9: bool, num: usize, access_time_fn: A, read_fn: R, write_fn: W)
        where A: Fn(&mut HW, AccessType, u32) -> usize, R: Fn(&mut HW, u32) -> T, W: Fn(&mut HW, u32, T) {
        let channel = self.get_channel(is_nds9, num);
        let count = channel.count_latch;
        let mut src_addr = channel.sad_latch;
        let mut dest_addr = channel.dad_latch;
        let src_addr_ctrl = channel.cnt.src_addr_ctrl;
        let dest_addr_ctrl = channel.cnt.dest_addr_ctrl;
        let transfer_32 = channel.cnt.transfer_32;
        let irq = channel.cnt.irq;
        channel.cnt.enable = channel.cnt.start_timing != DMAOccasion::Immediate && channel.cnt.repeat;
        info!("Running {:?} ARM{} DMA{}: Writing {} values to {:08X} from {:08X}, size: {}", channel.cnt.start_timing,
        if is_nds9 { 9 } else { 7 }, num, count, dest_addr, src_addr, if transfer_32 { 32 } else { 16 });

        let (addr_change, addr_mask) = if transfer_32 { (4, 0x3) } else { (2, 0x1) };
        src_addr &= !addr_mask;
        dest_addr &= !addr_mask;
        let mut first = true;
        let original_dest_addr = dest_addr;
        let mut cycles_passed = 0;
        for _ in 0..count {
            let cycle_type = if first { AccessType::N } else { AccessType::S };
            cycles_passed += access_time_fn(self, cycle_type, src_addr);
            cycles_passed += access_time_fn(self, cycle_type, dest_addr);
            let value = read_fn(self, src_addr);
            write_fn(self, dest_addr, value);

            src_addr = match src_addr_ctrl {
                0 => src_addr.wrapping_add(addr_change),
                1 => src_addr.wrapping_sub(addr_change),
                2 => src_addr,
                _ => panic!("Invalid DMA Source Address Control!"),
            };
            dest_addr = match dest_addr_ctrl {
                0 | 3 => dest_addr.wrapping_add(addr_change),
                1 => dest_addr.wrapping_sub(addr_change),
                2 => dest_addr,
                _ => unreachable!(),
            };
            first = false;
        }
        let channel = self.get_channel(is_nds9, num);
        channel.sad_latch = src_addr;
        channel.dad_latch = dest_addr;
        // if channel.cnt.enable { channel.count_latch = channel.count.count as u32 } // Only reload Count - TODO: Why?
        if dest_addr_ctrl == 3 { channel.dad_latch = original_dest_addr }
        cycles_passed += 2; // 2 I cycles

        if !channel.cnt.enable {
            if is_nds9 { self.dma9.disable(num) }
            else { self.dma7.disable(num) }
        }

        // TODO: Don't halt CPU if PC is in TCM
        self.clock(cycles_passed);
        
        if irq {
            let interrupt = match num {
                0 => InterruptRequest::DMA0,
                1 => InterruptRequest::DMA1,
                2 => InterruptRequest::DMA2,
                3 => InterruptRequest::DMA3,
                _ => unreachable!(),
            };
            self.interrupts[0].request |= interrupt;
            self.interrupts[1].request |= interrupt;
        }
    }

    fn check_dispstats<F>(&mut self, check: &mut F) where F: FnMut(&mut DISPSTAT, &mut InterruptController) {
        check(&mut self.gpu.dispstat7, &mut self.interrupts[0]);
        check(&mut self.gpu.dispstat9, &mut self.interrupts[1]);
    }
}

pub struct Scheduler {
    pub cycle: usize,
    event_queue: PriorityQueue<EventWrapper, Reverse<usize>>,
}

impl Scheduler {
    pub fn new() -> Scheduler {
        let queue = PriorityQueue::new();
        Scheduler {
            cycle: 0,
            event_queue: queue,
        }
    }

    fn get_next_event(&mut self) -> Option<EventWrapper> {
        // There should always be at least one event in the queue
        let (_event_type, cycle) = self.event_queue.peek().unwrap();
        if Reverse(self.cycle) <= *cycle {
            Some(self.event_queue.pop().unwrap().0)
        } else { None }
    }

    pub fn schedule(&mut self, event: Event, delay: usize) {
        let wrapper = EventWrapper::new(event, HW::handle_event);
        self.event_queue.push(wrapper, Reverse(self.cycle + delay));
    }

    pub fn run_now(&mut self, event: Event) {
        self.schedule(event, 0);
    }

    pub fn remove(&mut self, event: Event) {
        let wrapper = EventWrapper::new(event, HW::handle_event);
        self.event_queue.remove(&wrapper);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    DMA(bool, usize),
    StartNextLine,
    HBlank,
    VBlank,
    TimerOverflow(bool, usize),
    ROMWordTransfered,
    ROMBlockEnded(bool),
    GenerateAudioSample,
    StepAudioChannel(spu::ChannelSpec),
    ResetAudioChannel(spu::ChannelSpec),
}

struct EventWrapper {
    event: Event,
    handler: fn(&mut HW, Event),
}

impl EventWrapper {
    pub fn new(event: Event, handler: fn(&mut HW, Event)) -> Self {
        EventWrapper {
            event,
            handler,
        }
    }
}

impl PartialEq for EventWrapper {
    fn eq(&self, other: &Self) -> bool {
        self.event.eq(&other.event)
    }
}

impl Eq for EventWrapper {}

impl Hash for EventWrapper {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.event.hash(state);
    }
}
