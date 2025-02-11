//! Platform-Level Interrupt Controller (PLIC) peripheral.
//!
//! Specification: <https://github.com/riscv/riscv-plic-spec/blob/master/riscv-plic.adoc>

pub use super::PLIC;
use crate::register::mie;
use core::ops::Deref;
use volatile_register::{RO, RW};

/// Maximum number of interrupt sources supported by the PLIC standard.
const MAX_SOURCES: usize = 1_024;
/// Maximum number of words needed to represent interrupts with flags.
const MAX_FLAGS_WORDS: usize = MAX_SOURCES / (u32::BITS as usize);
/// Maximum number of contexts supported by the PLIC standard.
const MAX_CONTEXTS: usize = 15_872;

/// Register block.
#[repr(C)]
pub struct RegisterBlock {
    /// `0x0000_0000..=0x0000_0FFC` - Interrupt Priority Register.
    pub priority: [RW<u32>; MAX_SOURCES],
    /// `0x0000_1000..=0x0000_107C` - Interrupt Pending Register.
    pub pending: [RO<u32>; MAX_FLAGS_WORDS],
    /// `0x0000_1080..=0x0000_1FFC` - Reserved.
    _reserved1: [u32; 0x03e0],
    /// `0x0000_2000..=0x001F_1FFC` - Enable Registers (one per context).
    pub enables: [ContextEnable; MAX_CONTEXTS],
    /// `0x001F_2000..=0x001F_FFFF` - Reserved.
    _reserved2: [u32; 0x3800],
    /// `0x0020_0000..=0x03FF_FFFC` - State Registers (one per context).
    pub states: [ContextState; MAX_CONTEXTS],
}

/// Interrupt enable for a given context.
pub type ContextEnable = [RW<u32>; MAX_FLAGS_WORDS];

/// State of a single context.
#[repr(C)]
pub struct ContextState {
    /// `0x0000_0000` - Priority Threshold Register.
    pub threshold: RW<u32>,
    /// `0x0000_0004` - Claim/Complete Register.
    pub claim_complete: RW<u32>,
    /// `0x0000_0008..=0x0000_0FFC` - Reserved.
    _reserved: [u32; 0x3fe],
}

impl<const BASE: usize, const CONTEXT: usize> PLIC<BASE, CONTEXT> {
    /// Sets the Machine External Interrupt bit of the [`crate::register::mie`] CSR.
    /// This bit must be set for the PLIC to trigger machine external interrupts.
    #[inline]
    pub fn enable() {
        // SAFETY: atomic CSRRS instruction with no side effects
        unsafe { mie::set_mext() };
    }

    /// Clears the Machine External Interrupt bit of the [`crate::register::mie`] CSR.
    /// When cleared, the PLIC does not trigger machine external interrupts.
    #[inline]
    pub fn disable() {
        // SAFETY: atomic CSRRC instruction with no side effects
        unsafe { mie::clear_mext() };
    }

    /// Returns the priority level associated to a given interrupt source.
    #[inline]
    pub fn priority<I: InterruptNumber, P: PriorityNumber>(source: I) -> P {
        let source = usize::from(source.number());
        // SAFETY: atomic read with no side effects
        let priority = unsafe { (*Self::PTR).priority[source].read() } as _;
        P::try_from(priority).unwrap()
    }

    /// Sets the priority level of a given interrupt source.
    ///
    /// # Note
    ///
    /// Interrupt source priorities are shared among all the contexts of the PLIC.
    /// Thus, changing the priority of sources  may affect other PLIC contexts.
    ///
    /// # Safety
    ///
    /// Changing priority levels can break priority-based critical sections and compromise memory safety.
    #[inline]
    pub unsafe fn set_priority<I: InterruptNumber, P: PriorityNumber>(
        &mut self,
        source: I,
        priority: P,
    ) {
        let source = usize::from(source.number());
        let priority = priority.number().into();
        // SAFETY: atomic write with no side effects
        (*Self::PTR).priority[source].write(priority);
    }

    /// Checks if an interrupt triggered by a given source is pending.
    #[inline]
    pub fn is_interrupt_pending<I: InterruptNumber>(source: I) -> bool {
        let source = usize::from(source.number());
        let mask: u32 = 1 << (source % MAX_FLAGS_WORDS);
        // SAFETY: atomic read with no side effects
        let flags = unsafe { (*Self::PTR).pending[source / MAX_FLAGS_WORDS].read() };
        (flags & mask) == mask
    }

    /// Checks if an interrupt source is enabled for the PLIC context.
    #[inline]
    pub fn is_interrupt_enabled<I: InterruptNumber>(source: I) -> bool {
        let source = usize::from(source.number());
        let mask: u32 = 1 << (source % MAX_FLAGS_WORDS);
        // SAFETY: atomic read with no side effects
        let flags = unsafe { (*Self::PTR).enables[CONTEXT][source / MAX_FLAGS_WORDS].read() };
        (flags & mask) == mask
    }

    /// Enables an interrupt source for the PLIC context.
    ///
    /// # Safety
    ///
    /// It performs non-atomic read-modify-write operations, which may lead to undefined behavior.
    /// Additionally, Enabling an interrupt source can break mask-based critical sections.
    #[inline]
    pub unsafe fn enable_interrupt<I: InterruptNumber>(&mut self, source: I) {
        let source = usize::from(source.number());
        let mask: u32 = 1 << (source % MAX_FLAGS_WORDS);
        self.enables[CONTEXT][source / MAX_FLAGS_WORDS].modify(|value| value | mask);
    }

    /// Disables an interrupt source for the PLIC context.
    ///
    /// # Safety
    ///
    /// It performs non-atomic read-modify-write operations, which may lead to undefined behavior.
    #[inline]
    pub unsafe fn disable_interrupt<I: InterruptNumber>(&mut self, source: I) {
        let source = usize::from(source.number());
        let mask: u32 = 1 << (source % MAX_FLAGS_WORDS);
        self.enables[CONTEXT][source / MAX_FLAGS_WORDS].modify(|value| value & !mask);
    }

    /// Returns the priority threshold of the PLIC context.
    #[inline]
    pub fn threshold<P: PriorityNumber>() -> P {
        // SAFETY: atomic read with no side effects
        let priority = unsafe { (*Self::PTR).states[CONTEXT].threshold.read() } as _;
        P::try_from(priority).unwrap()
    }

    /// Sets the priority threshold for for the PLIC context.
    ///
    /// # Safety
    ///
    /// Unmasking an interrupt source can break mask-based critical sections.
    #[inline]
    pub unsafe fn set_threshold<P: PriorityNumber>(&mut self, priority: P) {
        let priority = priority.number().into();
        // SAFETY: atomic write with no side effects
        (*Self::PTR).states[CONTEXT].threshold.write(priority);
    }

    /// Claims the number of a pending interrupt for for the PLIC context.
    /// If no interrupt is pending for this context, it returns [`None`].
    #[inline]
    pub fn claim<I: InterruptNumber>() -> Option<I> {
        // SAFETY: atomic read with no side effects
        let interrupt = unsafe { (*Self::PTR).states[CONTEXT].claim_complete.read() } as _;
        match interrupt {
            0 => None,
            i => Some(I::try_from(i).unwrap()),
        }
    }

    /// Marks a pending interrupt as complete from for the PLIC context.
    ///
    /// # Note
    ///
    /// If the source ID does not match an interrupt source that is
    /// currently enabled for the target, the completion is silently ignored.
    #[inline]
    pub fn complete<I: InterruptNumber>(source: I) {
        let source = source.number().into();
        // SAFETY: atomic write with no side effects
        unsafe {
            (*Self::PTR).states[CONTEXT].claim_complete.write(source);
        }
    }

    /// Resets the PLIC peripherals. Namely, it performs the following operations:
    ///
    /// - Sets PLIC context threshold to the maximum interrupt level (i.e., never interrupt).
    /// - Disables all the interrupt sources.
    /// - Sets interrupt source priority to 0 (i.e., no interrupt).
    ///
    /// # Safety
    ///
    /// It performs non-atomic read-modify-write operations, which may lead to undefined behavior.
    #[inline(always)]
    pub unsafe fn reset<I: InterruptNumber, P: PriorityNumber>(&mut self) {
        self.set_threshold(P::try_from(P::MAX_PRIORITY_NUMBER).unwrap());
        let no_interrupt = P::try_from(0).unwrap();
        for source in (1..=I::MAX_INTERRUPT_NUMBER).filter_map(|n| I::try_from(n).ok()) {
            self.disable_interrupt(source);
            self.set_priority(source, no_interrupt);
        }
    }
}

impl<const BASE: usize, const CONTEXT: usize> Deref for PLIC<BASE, CONTEXT> {
    type Target = RegisterBlock;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { &*Self::PTR }
    }
}

unsafe impl<const BASE: usize, const CONTEXT: usize> Send for PLIC<BASE, CONTEXT> {}

/// Trait for enums of interrupt numbers.
///
/// This trait should be implemented by a peripheral access crate (PAC)
/// on its enum of available external interrupts for a specific device.
/// Each variant must convert to a `u16` of its interrupt number.
///
/// # Note
///
/// Recall that the interrupt number `0` is reserved as "no interrupt".
///
/// # Safety
///
/// This trait must only be implemented on enums of external interrupts. Each
/// enum variant must represent a distinct value (no duplicates are permitted),
/// and must always return the same value (do not change at runtime).
/// All the interrupt numbers must be less than or equal to `MAX_INTERRUPT_NUMBER`.
/// `MAX_INTERRUPT_NUMBER` must coincide with the highest allowed interrupt number.
///
/// These requirements ensure safe nesting of critical sections.
pub unsafe trait InterruptNumber: Copy {
    /// Highest number assigned to an interrupt source.
    const MAX_INTERRUPT_NUMBER: u16;

    /// Converts an interrupt source to its corresponding number.
    fn number(self) -> u16;

    /// Tries to convert a number to a valid interrupt source.
    /// If the conversion fails, it returns an error with the number back.
    fn try_from(value: u16) -> Result<Self, u16>;
}

/// Trait for enums of interrupt priority numbers.
///
/// This trait should be implemented by a peripheral access crate (PAC)
/// on its enum of available priority numbers for a specific device.
/// Each variant must convert to a `u8` of its priority level.
///
/// # Note
///
/// Recall that the priority number `0` is reserved as "never interrupt".
///
/// # Safety
///
/// This trait must only be implemented on enums of priority levels. Each
/// enum variant must represent a distinct value (no duplicates are permitted),
/// and must always return the same value (do not change at runtime).
/// There must be a valid priority number set to 0 (i.e., never interrupt).
/// All the priority level numbers must be less than or equal to `MAX_PRIORITY_NUMBER`.
/// `MAX_PRIORITY_NUMBER` must coincide with the highest allowed priority number.
///
/// These requirements ensure safe nesting of critical sections.
pub unsafe trait PriorityNumber: Copy {
    /// Number assigned to the highest priority level.
    const MAX_PRIORITY_NUMBER: u8;

    /// Converts a priority level to its corresponding number.
    fn number(self) -> u8;

    /// Tries to convert a number to a valid priority level.
    /// If the conversion fails, it returns an error with the number back.
    fn try_from(value: u8) -> Result<Self, u8>;
}
