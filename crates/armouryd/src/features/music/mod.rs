//! Music-reactive lighting: PipeWire capture → FFT → per-key frames over the N-KEY hidraw node.
pub mod perkey;
pub mod analyze;
pub mod render;
pub mod worker;
