//! In-crate test suites: one file per parser area plus the golden smoke.

mod collector;

mod declarations;
mod directives;
mod drain;
mod emitter_w1;
mod emitter_w2;
mod expressions;
mod features;
mod gate_1315_p3_families;
mod gate_1600_declaring_parameters;
mod gate_351;
mod gate_410;
mod gate_445;
mod gate_acceptance;
mod gate_bi_name_faces;
mod gate_dark_pins;
mod gate_fam11_restricted_patterns;
mod gate_fp_pins;
mod gate_local_pins;
mod gate_m0;
mod gate_m0_wave1;
mod gate_m0_wave10;
mod gate_m0_wave11;
mod gate_m0_wave2;
mod gate_m0_wave3;
mod gate_m0_wave4;
mod gate_m0_wave5;
mod gate_m0_wave6;
mod gate_m0_wave7;
mod gate_m0_wave8;
mod gate_m0_wave9;
mod gate_pattern_const_shorthand;
mod gate_ppin;
mod gate_rc2200_recovery;
mod patterns;

mod recovery;
mod statements;
mod termination;
mod type_compute;
mod types;
