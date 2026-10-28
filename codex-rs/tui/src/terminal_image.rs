//! Shared terminal image transports for conversation images and optional pets.

pub(crate) mod image_protocol;
pub(crate) mod sixel;

pub(crate) use image_protocol::ImageProtocol;
pub(crate) use image_protocol::PetImageSupport;
pub(crate) use image_protocol::detect_pet_image_support;
pub(crate) use image_protocol::kitty_delete_image;
pub(crate) use image_protocol::kitty_transmit_png_file_with_id;
pub(crate) use image_protocol::kitty_transmit_png_with_id;
pub(crate) use sixel::encode_rgba;
