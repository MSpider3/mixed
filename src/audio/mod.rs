pub mod player;
pub mod visualizer;

#[cfg(not(target_os = "android"))]
pub mod viz_source;

#[cfg(not(target_os = "android"))]
pub mod rodio_backend;

#[cfg(not(target_os = "android"))]
pub mod symphonia_source;

#[cfg(target_os = "android")]
pub mod mpv_backend;
