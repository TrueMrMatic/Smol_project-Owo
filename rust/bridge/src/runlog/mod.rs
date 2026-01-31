#[cfg(feature = "runlog")]
mod full;
#[cfg(not(feature = "runlog"))]
mod stub;

#[cfg(feature = "runlog")]
pub use full::*;
#[cfg(not(feature = "runlog"))]
pub use stub::*;
