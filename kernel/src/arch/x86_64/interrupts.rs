//! Per-vector exception/interrupt entry stubs.
//!
//! The CPU's own hardware trap frame has two different shapes: ten vectors
//! (8, 10, 11, 12, 13, 14, 17, 21, 29, 30; Intel SDM Vol. 3A 6.15) push a
//! 32-bit error code before `RIP`; the rest don't. `define_stub!` below
//! normalizes that: it emits one `#[unsafe(naked)] unsafe extern "C" fn`
//! per vector that pushes a dummy 0 error code when the CPU didn't supply
//! one, then always pushes the vector number and jumps to the shared
//! `common_stub`, which pushes the 15 general-purpose registers, calls
//! `trap_dispatch` with `rsp` (now a valid `*mut TrapFrame`, see trap.rs)
//! in `rdi`, pops the registers back, drops the vector+error_code pair,
//! and `iretq`s.
//!
//! Push order matters: `common_stub` pushes `rax` first and `r15` last, so
//! the *last* value pushed -- `r15` -- ends up at the lowest address, i.e.
//! at `rsp` itself once `mov rdi, rsp` runs. That is exactly
//! `TrapFrame`'s field order (`r15` first, `rax` last; see trap.rs):
//! reinterpreting `rdi` as `*mut TrapFrame` is valid because the struct
//! layout mirrors the stack layout byte for byte.
//!
//! Alignment: the CPU aligns `rsp` to 16 before pushing its 5- or 6-word
//! frame; with our dummy error code both shapes leave `rsp` a multiple of
//! 16. The vector push (+8 bytes) makes it 8 mod 16; the 15 GPR pushes
//! (120 bytes, itself 8 mod 16) bring it back to a multiple of 16, so
//! `call {dispatch}` -- which itself pushes an 8-byte return address --
//! enters `trap_dispatch` with `rsp` at 8 mod 16, exactly the SysV ABI's
//! requirement at function entry.
//!
//! 256 distinct functions (rather than one generic one) are what let each
//! stub embed its own vector number as an immediate; `define_stub!` is
//! invoked once per vector below so the only thing that varies line to
//! line is the vector number and whether the CPU supplies an error code.

use crate::arch::x86_64::trap::trap_dispatch;

/// The shared second half of every stub: saves the 15 GPRs, calls
/// `trap_dispatch`, restores them, discards the vector+error_code pair the
/// per-vector stub pushed, and returns from the interrupt.
#[unsafe(naked)]
unsafe extern "C" fn common_stub() {
    core::arch::naked_asm!(
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "call {dispatch}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "add rsp, 16",
        "iretq",
        dispatch = sym trap_dispatch,
    );
}

/// Defines one per-vector naked stub. `err` vectors leave the CPU's own
/// error code in place; `noerr` vectors push a dummy 0 so every vector
/// reaches `common_stub` with the same stack shape.
macro_rules! define_stub {
    ($name:ident, $vec:literal, err) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                "push {vec}",
                "jmp {common}",
                vec = const $vec,
                common = sym common_stub,
            );
        }
    };
    ($name:ident, $vec:literal, noerr) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                "push 0",
                "push {vec}",
                "jmp {common}",
                vec = const $vec,
                common = sym common_stub,
            );
        }
    };
}

define_stub!(stub_0, 0, noerr);
define_stub!(stub_1, 1, noerr);
define_stub!(stub_2, 2, noerr);
define_stub!(stub_3, 3, noerr);
define_stub!(stub_4, 4, noerr);
define_stub!(stub_5, 5, noerr);
define_stub!(stub_6, 6, noerr);
define_stub!(stub_7, 7, noerr);
define_stub!(stub_8, 8, err);
define_stub!(stub_9, 9, noerr);
define_stub!(stub_10, 10, err);
define_stub!(stub_11, 11, err);
define_stub!(stub_12, 12, err);
define_stub!(stub_13, 13, err);
define_stub!(stub_14, 14, err);
define_stub!(stub_15, 15, noerr);
define_stub!(stub_16, 16, noerr);
define_stub!(stub_17, 17, err);
define_stub!(stub_18, 18, noerr);
define_stub!(stub_19, 19, noerr);
define_stub!(stub_20, 20, noerr);
define_stub!(stub_21, 21, err);
define_stub!(stub_22, 22, noerr);
define_stub!(stub_23, 23, noerr);
define_stub!(stub_24, 24, noerr);
define_stub!(stub_25, 25, noerr);
define_stub!(stub_26, 26, noerr);
define_stub!(stub_27, 27, noerr);
define_stub!(stub_28, 28, noerr);
define_stub!(stub_29, 29, err);
define_stub!(stub_30, 30, err);
define_stub!(stub_31, 31, noerr);
define_stub!(stub_32, 32, noerr);
define_stub!(stub_33, 33, noerr);
define_stub!(stub_34, 34, noerr);
define_stub!(stub_35, 35, noerr);
define_stub!(stub_36, 36, noerr);
define_stub!(stub_37, 37, noerr);
define_stub!(stub_38, 38, noerr);
define_stub!(stub_39, 39, noerr);
define_stub!(stub_40, 40, noerr);
define_stub!(stub_41, 41, noerr);
define_stub!(stub_42, 42, noerr);
define_stub!(stub_43, 43, noerr);
define_stub!(stub_44, 44, noerr);
define_stub!(stub_45, 45, noerr);
define_stub!(stub_46, 46, noerr);
define_stub!(stub_47, 47, noerr);
define_stub!(stub_48, 48, noerr);
define_stub!(stub_49, 49, noerr);
define_stub!(stub_50, 50, noerr);
define_stub!(stub_51, 51, noerr);
define_stub!(stub_52, 52, noerr);
define_stub!(stub_53, 53, noerr);
define_stub!(stub_54, 54, noerr);
define_stub!(stub_55, 55, noerr);
define_stub!(stub_56, 56, noerr);
define_stub!(stub_57, 57, noerr);
define_stub!(stub_58, 58, noerr);
define_stub!(stub_59, 59, noerr);
define_stub!(stub_60, 60, noerr);
define_stub!(stub_61, 61, noerr);
define_stub!(stub_62, 62, noerr);
define_stub!(stub_63, 63, noerr);
define_stub!(stub_64, 64, noerr);
define_stub!(stub_65, 65, noerr);
define_stub!(stub_66, 66, noerr);
define_stub!(stub_67, 67, noerr);
define_stub!(stub_68, 68, noerr);
define_stub!(stub_69, 69, noerr);
define_stub!(stub_70, 70, noerr);
define_stub!(stub_71, 71, noerr);
define_stub!(stub_72, 72, noerr);
define_stub!(stub_73, 73, noerr);
define_stub!(stub_74, 74, noerr);
define_stub!(stub_75, 75, noerr);
define_stub!(stub_76, 76, noerr);
define_stub!(stub_77, 77, noerr);
define_stub!(stub_78, 78, noerr);
define_stub!(stub_79, 79, noerr);
define_stub!(stub_80, 80, noerr);
define_stub!(stub_81, 81, noerr);
define_stub!(stub_82, 82, noerr);
define_stub!(stub_83, 83, noerr);
define_stub!(stub_84, 84, noerr);
define_stub!(stub_85, 85, noerr);
define_stub!(stub_86, 86, noerr);
define_stub!(stub_87, 87, noerr);
define_stub!(stub_88, 88, noerr);
define_stub!(stub_89, 89, noerr);
define_stub!(stub_90, 90, noerr);
define_stub!(stub_91, 91, noerr);
define_stub!(stub_92, 92, noerr);
define_stub!(stub_93, 93, noerr);
define_stub!(stub_94, 94, noerr);
define_stub!(stub_95, 95, noerr);
define_stub!(stub_96, 96, noerr);
define_stub!(stub_97, 97, noerr);
define_stub!(stub_98, 98, noerr);
define_stub!(stub_99, 99, noerr);
define_stub!(stub_100, 100, noerr);
define_stub!(stub_101, 101, noerr);
define_stub!(stub_102, 102, noerr);
define_stub!(stub_103, 103, noerr);
define_stub!(stub_104, 104, noerr);
define_stub!(stub_105, 105, noerr);
define_stub!(stub_106, 106, noerr);
define_stub!(stub_107, 107, noerr);
define_stub!(stub_108, 108, noerr);
define_stub!(stub_109, 109, noerr);
define_stub!(stub_110, 110, noerr);
define_stub!(stub_111, 111, noerr);
define_stub!(stub_112, 112, noerr);
define_stub!(stub_113, 113, noerr);
define_stub!(stub_114, 114, noerr);
define_stub!(stub_115, 115, noerr);
define_stub!(stub_116, 116, noerr);
define_stub!(stub_117, 117, noerr);
define_stub!(stub_118, 118, noerr);
define_stub!(stub_119, 119, noerr);
define_stub!(stub_120, 120, noerr);
define_stub!(stub_121, 121, noerr);
define_stub!(stub_122, 122, noerr);
define_stub!(stub_123, 123, noerr);
define_stub!(stub_124, 124, noerr);
define_stub!(stub_125, 125, noerr);
define_stub!(stub_126, 126, noerr);
define_stub!(stub_127, 127, noerr);
define_stub!(stub_128, 128, noerr);
define_stub!(stub_129, 129, noerr);
define_stub!(stub_130, 130, noerr);
define_stub!(stub_131, 131, noerr);
define_stub!(stub_132, 132, noerr);
define_stub!(stub_133, 133, noerr);
define_stub!(stub_134, 134, noerr);
define_stub!(stub_135, 135, noerr);
define_stub!(stub_136, 136, noerr);
define_stub!(stub_137, 137, noerr);
define_stub!(stub_138, 138, noerr);
define_stub!(stub_139, 139, noerr);
define_stub!(stub_140, 140, noerr);
define_stub!(stub_141, 141, noerr);
define_stub!(stub_142, 142, noerr);
define_stub!(stub_143, 143, noerr);
define_stub!(stub_144, 144, noerr);
define_stub!(stub_145, 145, noerr);
define_stub!(stub_146, 146, noerr);
define_stub!(stub_147, 147, noerr);
define_stub!(stub_148, 148, noerr);
define_stub!(stub_149, 149, noerr);
define_stub!(stub_150, 150, noerr);
define_stub!(stub_151, 151, noerr);
define_stub!(stub_152, 152, noerr);
define_stub!(stub_153, 153, noerr);
define_stub!(stub_154, 154, noerr);
define_stub!(stub_155, 155, noerr);
define_stub!(stub_156, 156, noerr);
define_stub!(stub_157, 157, noerr);
define_stub!(stub_158, 158, noerr);
define_stub!(stub_159, 159, noerr);
define_stub!(stub_160, 160, noerr);
define_stub!(stub_161, 161, noerr);
define_stub!(stub_162, 162, noerr);
define_stub!(stub_163, 163, noerr);
define_stub!(stub_164, 164, noerr);
define_stub!(stub_165, 165, noerr);
define_stub!(stub_166, 166, noerr);
define_stub!(stub_167, 167, noerr);
define_stub!(stub_168, 168, noerr);
define_stub!(stub_169, 169, noerr);
define_stub!(stub_170, 170, noerr);
define_stub!(stub_171, 171, noerr);
define_stub!(stub_172, 172, noerr);
define_stub!(stub_173, 173, noerr);
define_stub!(stub_174, 174, noerr);
define_stub!(stub_175, 175, noerr);
define_stub!(stub_176, 176, noerr);
define_stub!(stub_177, 177, noerr);
define_stub!(stub_178, 178, noerr);
define_stub!(stub_179, 179, noerr);
define_stub!(stub_180, 180, noerr);
define_stub!(stub_181, 181, noerr);
define_stub!(stub_182, 182, noerr);
define_stub!(stub_183, 183, noerr);
define_stub!(stub_184, 184, noerr);
define_stub!(stub_185, 185, noerr);
define_stub!(stub_186, 186, noerr);
define_stub!(stub_187, 187, noerr);
define_stub!(stub_188, 188, noerr);
define_stub!(stub_189, 189, noerr);
define_stub!(stub_190, 190, noerr);
define_stub!(stub_191, 191, noerr);
define_stub!(stub_192, 192, noerr);
define_stub!(stub_193, 193, noerr);
define_stub!(stub_194, 194, noerr);
define_stub!(stub_195, 195, noerr);
define_stub!(stub_196, 196, noerr);
define_stub!(stub_197, 197, noerr);
define_stub!(stub_198, 198, noerr);
define_stub!(stub_199, 199, noerr);
define_stub!(stub_200, 200, noerr);
define_stub!(stub_201, 201, noerr);
define_stub!(stub_202, 202, noerr);
define_stub!(stub_203, 203, noerr);
define_stub!(stub_204, 204, noerr);
define_stub!(stub_205, 205, noerr);
define_stub!(stub_206, 206, noerr);
define_stub!(stub_207, 207, noerr);
define_stub!(stub_208, 208, noerr);
define_stub!(stub_209, 209, noerr);
define_stub!(stub_210, 210, noerr);
define_stub!(stub_211, 211, noerr);
define_stub!(stub_212, 212, noerr);
define_stub!(stub_213, 213, noerr);
define_stub!(stub_214, 214, noerr);
define_stub!(stub_215, 215, noerr);
define_stub!(stub_216, 216, noerr);
define_stub!(stub_217, 217, noerr);
define_stub!(stub_218, 218, noerr);
define_stub!(stub_219, 219, noerr);
define_stub!(stub_220, 220, noerr);
define_stub!(stub_221, 221, noerr);
define_stub!(stub_222, 222, noerr);
define_stub!(stub_223, 223, noerr);
define_stub!(stub_224, 224, noerr);
define_stub!(stub_225, 225, noerr);
define_stub!(stub_226, 226, noerr);
define_stub!(stub_227, 227, noerr);
define_stub!(stub_228, 228, noerr);
define_stub!(stub_229, 229, noerr);
define_stub!(stub_230, 230, noerr);
define_stub!(stub_231, 231, noerr);
define_stub!(stub_232, 232, noerr);
define_stub!(stub_233, 233, noerr);
define_stub!(stub_234, 234, noerr);
define_stub!(stub_235, 235, noerr);
define_stub!(stub_236, 236, noerr);
define_stub!(stub_237, 237, noerr);
define_stub!(stub_238, 238, noerr);
define_stub!(stub_239, 239, noerr);
define_stub!(stub_240, 240, noerr);
define_stub!(stub_241, 241, noerr);
define_stub!(stub_242, 242, noerr);
define_stub!(stub_243, 243, noerr);
define_stub!(stub_244, 244, noerr);
define_stub!(stub_245, 245, noerr);
define_stub!(stub_246, 246, noerr);
define_stub!(stub_247, 247, noerr);
define_stub!(stub_248, 248, noerr);
define_stub!(stub_249, 249, noerr);
define_stub!(stub_250, 250, noerr);
define_stub!(stub_251, 251, noerr);
define_stub!(stub_252, 252, noerr);
define_stub!(stub_253, 253, noerr);
define_stub!(stub_254, 254, noerr);
define_stub!(stub_255, 255, noerr);

/// One entry point per IDT vector, in vector order; `idt::init()` points
/// gate `v` at `STUBS[v]`.
pub(crate) static STUBS: [unsafe extern "C" fn(); 256] = [
    stub_0, stub_1, stub_2, stub_3, stub_4, stub_5, stub_6, stub_7,
    stub_8, stub_9, stub_10, stub_11, stub_12, stub_13, stub_14, stub_15,
    stub_16, stub_17, stub_18, stub_19, stub_20, stub_21, stub_22, stub_23,
    stub_24, stub_25, stub_26, stub_27, stub_28, stub_29, stub_30, stub_31,
    stub_32, stub_33, stub_34, stub_35, stub_36, stub_37, stub_38, stub_39,
    stub_40, stub_41, stub_42, stub_43, stub_44, stub_45, stub_46, stub_47,
    stub_48, stub_49, stub_50, stub_51, stub_52, stub_53, stub_54, stub_55,
    stub_56, stub_57, stub_58, stub_59, stub_60, stub_61, stub_62, stub_63,
    stub_64, stub_65, stub_66, stub_67, stub_68, stub_69, stub_70, stub_71,
    stub_72, stub_73, stub_74, stub_75, stub_76, stub_77, stub_78, stub_79,
    stub_80, stub_81, stub_82, stub_83, stub_84, stub_85, stub_86, stub_87,
    stub_88, stub_89, stub_90, stub_91, stub_92, stub_93, stub_94, stub_95,
    stub_96, stub_97, stub_98, stub_99, stub_100, stub_101, stub_102, stub_103,
    stub_104, stub_105, stub_106, stub_107, stub_108, stub_109, stub_110, stub_111,
    stub_112, stub_113, stub_114, stub_115, stub_116, stub_117, stub_118, stub_119,
    stub_120, stub_121, stub_122, stub_123, stub_124, stub_125, stub_126, stub_127,
    stub_128, stub_129, stub_130, stub_131, stub_132, stub_133, stub_134, stub_135,
    stub_136, stub_137, stub_138, stub_139, stub_140, stub_141, stub_142, stub_143,
    stub_144, stub_145, stub_146, stub_147, stub_148, stub_149, stub_150, stub_151,
    stub_152, stub_153, stub_154, stub_155, stub_156, stub_157, stub_158, stub_159,
    stub_160, stub_161, stub_162, stub_163, stub_164, stub_165, stub_166, stub_167,
    stub_168, stub_169, stub_170, stub_171, stub_172, stub_173, stub_174, stub_175,
    stub_176, stub_177, stub_178, stub_179, stub_180, stub_181, stub_182, stub_183,
    stub_184, stub_185, stub_186, stub_187, stub_188, stub_189, stub_190, stub_191,
    stub_192, stub_193, stub_194, stub_195, stub_196, stub_197, stub_198, stub_199,
    stub_200, stub_201, stub_202, stub_203, stub_204, stub_205, stub_206, stub_207,
    stub_208, stub_209, stub_210, stub_211, stub_212, stub_213, stub_214, stub_215,
    stub_216, stub_217, stub_218, stub_219, stub_220, stub_221, stub_222, stub_223,
    stub_224, stub_225, stub_226, stub_227, stub_228, stub_229, stub_230, stub_231,
    stub_232, stub_233, stub_234, stub_235, stub_236, stub_237, stub_238, stub_239,
    stub_240, stub_241, stub_242, stub_243, stub_244, stub_245, stub_246, stub_247,
    stub_248, stub_249, stub_250, stub_251, stub_252, stub_253, stub_254, stub_255,
];
