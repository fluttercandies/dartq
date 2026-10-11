//! In-crate test suites: one file per parser area plus the golden smoke.

mod collector;

mod declarations;
mod directives;
mod drain;
mod expressions;
mod features;
mod gate_351;
mod gate_410;
mod gate_445;
mod gate_m0;
mod gate_m0_wave1;
mod gate_m0_wave10;
mod gate_m0_wave2;
mod gate_m0_wave3;
mod gate_m0_wave4;
mod gate_m0_wave5;
mod gate_m0_wave6;
mod gate_m0_wave7;
mod gate_m0_wave8;
mod gate_m0_wave9;
mod goldens;
mod patterns;

mod recovery;
mod statements;
mod termination;
mod type_compute;
mod types;
