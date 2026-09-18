//! 流式事件与聚合模块入口

pub mod accumulator;
pub mod part;

pub use accumulator::StreamAccumulator;
pub use part::StreamPart;
