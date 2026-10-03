//! Helpers for textures built in code.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// RGBA8 image with a full box-filtered mip chain and anisotropic filtering — crisp up
/// close, no shimmering in the distance. `format` is sRGB for colours, linear for data
/// such as distance fields.
pub fn image_with_mips(
    w: usize,
    h: usize,
    base: Vec<u8>,
    format: TextureFormat,
    address_u: ImageAddressMode,
    address_v: ImageAddressMode,
) -> Image {
    let mut data = base.clone();
    let (mut lw, mut lh, mut level) = (w, h, base);
    let mut levels = 1;
    while lw > 1 || lh > 1 {
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = ((2 * x + dx).min(lw - 1), (2 * y + dy).min(lh - 1));
                        sum += level[(sy * lw + sx) * 4 + c] as u32;
                    }
                    next[(y * nw + x) * 4 + c] = (sum / 4) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        (lw, lh, level) = (nw, nh, next);
        levels += 1;
    }
    // `Image::new` only accepts the base level; mip levels follow it in `data`.
    let mut image = Image::new_uninit(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address_u,
        address_mode_v: address_v,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    image
}
