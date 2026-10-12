use windows::Win32::Graphics::Direct3D11::*;

use crate::shaders::*;
use crate::wcap::*;

use windows::Win32::Graphics::{
    Direct3D::{Fxc::D3DDecompressShaders, ID3DBlob, D3D_SRV_DIMENSION_TEXTURE2D},
    Dxgi::Common::*,
};

pub unsafe fn tex_resize_create(
    resize: &mut TexResize,
    device: &ID3D11Device,
    input_width: u32,
    input_height: u32,
    output_width: u32,
    output_height: u32,
    linear_space: bool,
    input_usage: D3D11_BIND_FLAG,
) {
    *resize = TexResize::default();
    let mut input_texture_desc = D3D11_TEXTURE2D_DESC::default();
    input_texture_desc.Width = input_width;
    input_texture_desc.Height = input_height;
    input_texture_desc.MipLevels = 1;
    input_texture_desc.ArraySize = 1;
    input_texture_desc.Format = DXGI_FORMAT_B8G8R8A8_TYPELESS;
    input_texture_desc.SampleDesc = DXGI_SAMPLE_DESC {
        Count: 1,
        Quality: 0,
    };
    input_texture_desc.Usage = D3D11_USAGE_DEFAULT;
    input_texture_desc.BindFlags = (input_usage | D3D11_BIND_SHADER_RESOURCE).0 as u32;

    device
        .CreateTexture2D(&input_texture_desc, None, Some(&mut resize.input_texture))
        .expect("CreateTexture2D failed");

    if input_width == output_width && input_height == output_height {
        resize.input_view_in = None;
        resize.output_texture = resize.input_texture.clone();
    } else {
        let mut shader: Option<ID3DBlob> = None;
        let mut shader_bytes: *const u8;
        let mut shader_size: usize;

        if linear_space {
            shader_bytes = RESIZE_LINEAR_PASS_H_SHADER_BYTES.as_ptr();
            shader_size = RESIZE_LINEAR_PASS_H_SHADER_BYTES.len();
        } else {
            shader_bytes = RESIZE_PASS_H_SHADER_BYTES.as_ptr();
            shader_size = RESIZE_PASS_H_SHADER_BYTES.len();
        }

        D3DDecompressShaders(
            shader_bytes as _,
            shader_size,
            1,
            0,
            None,
            0,
            &mut shader,
            None,
        )
        .expect("Shader decompression failed");

        if let Some(shader_ref) = shader.as_mut() {
            let shader_bytes = std::slice::from_raw_parts(
                shader_ref.GetBufferPointer() as *const u8,
                shader_ref.GetBufferSize(),
            );
            device
                .CreateComputeShader(shader_bytes, None, Some(&mut resize.pass_h))
                .expect("CreateComputeShader failed");
            drop(shader);
        }

        let mut shader: Option<ID3DBlob> = None;
        if linear_space {
            shader_bytes = RESIZE_LINEAR_PASS_V_SHADER_BYTES.as_ptr();
            shader_size = RESIZE_LINEAR_PASS_V_SHADER_BYTES.len();
        } else {
            shader_bytes = RESIZE_PASS_V_SHADER_BYTES.as_ptr();
            shader_size = RESIZE_PASS_V_SHADER_BYTES.len();
        }

        D3DDecompressShaders(
            shader_bytes as _,
            shader_size,
            1,
            0,
            None,
            0,
            &mut shader,
            None,
        )
        .expect("Shader decompression failed");

        if let Some(shader_ref) = shader.as_mut() {
            let shader_bytes = std::slice::from_raw_parts(
                shader_ref.GetBufferPointer() as *const u8,
                shader_ref.GetBufferSize(),
            );
            device
                .CreateComputeShader(shader_bytes, None, Some(&mut resize.pass_v))
                .expect("CreateComputeShader failed");
            drop(shader);
        }

        let mut view_in_desc = D3D11_SHADER_RESOURCE_VIEW_DESC::default();
        view_in_desc.Format = if linear_space {
            DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        } else {
            DXGI_FORMAT_B8G8R8A8_UNORM
        };
        view_in_desc.ViewDimension = D3D_SRV_DIMENSION_TEXTURE2D;
        view_in_desc.Anonymous.Texture2D.MipLevels = 0xFFFFFFFF;

        device
            .CreateShaderResourceView(
                resize.input_texture.as_ref().unwrap(),
                Some(&view_in_desc as *const D3D11_SHADER_RESOURCE_VIEW_DESC),
                Some(&mut resize.input_view_in),
            )
            .expect("CreateShaderResourceView failed");

        let mut output_texture_desc = D3D11_TEXTURE2D_DESC::default();
        output_texture_desc.Width = output_width;
        output_texture_desc.Height = output_height;
        output_texture_desc.MipLevels = 1;
        output_texture_desc.ArraySize = 1;
        output_texture_desc.Format = DXGI_FORMAT_B8G8R8A8_TYPELESS;
        output_texture_desc.SampleDesc = DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        };
        output_texture_desc.Usage = D3D11_USAGE_DEFAULT;
        output_texture_desc.BindFlags =
            (D3D11_BIND_UNORDERED_ACCESS | D3D11_BIND_SHADER_RESOURCE).0 as u32;

        device
            .CreateTexture2D(&output_texture_desc, None, Some(&mut resize.output_texture))
            .expect("CreateTexture2D failed");

        let mut view_out_desc = D3D11_UNORDERED_ACCESS_VIEW_DESC::default();
        view_out_desc.Format = DXGI_FORMAT_R32_UINT;
        view_out_desc.ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;
        view_out_desc.Anonymous.Texture2D.MipSlice = 0;

        device
            .CreateUnorderedAccessView(
                resize.output_texture.as_ref().unwrap(),
                Some(&view_out_desc as *const D3D11_UNORDERED_ACCESS_VIEW_DESC),
                Some(&mut resize.output_view_out),
            )
            .expect("CreateUnorderedAccessView failed");

        let mut middle_texture_desc = D3D11_TEXTURE2D_DESC::default();
        middle_texture_desc.Width = output_width;
        middle_texture_desc.Height = input_height;
        middle_texture_desc.MipLevels = 1;
        middle_texture_desc.ArraySize = 1;
        middle_texture_desc.Format = DXGI_FORMAT_B8G8R8A8_TYPELESS;
        middle_texture_desc.SampleDesc = DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        };
        middle_texture_desc.Usage = D3D11_USAGE_DEFAULT;
        middle_texture_desc.BindFlags =
            (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS).0 as u32;

        let mut middle_texture: Option<ID3D11Texture2D> = None;
        device
            .CreateTexture2D(&middle_texture_desc, None, Some(&mut middle_texture))
            .expect("CreateTexture2D failed");

        if let Some(middle_tex) = middle_texture.as_ref() {
            device
                .CreateShaderResourceView(
                    middle_tex,
                    Some(&view_in_desc as *const D3D11_SHADER_RESOURCE_VIEW_DESC),
                    Some(&mut resize.middle_view_in),
                )
                .expect("CreateShaderResourceView failed");
            device
                .CreateUnorderedAccessView(
                    middle_tex,
                    Some(&view_out_desc as *const D3D11_UNORDERED_ACCESS_VIEW_DESC),
                    Some(&mut resize.middle_view_out),
                )
                .expect("CreateUnorderedAccessView failed");
        }
    }

    resize.input_width = input_width;
    resize.input_height = input_height;
    resize.output_width = output_width;
    resize.output_height = output_height;
}

pub unsafe fn tex_resize_release(resize: &mut TexResize) {
    *resize = TexResize::default();
}

pub unsafe fn tex_resize_dispatch(resize: &mut TexResize, context: &ID3D11DeviceContext) {
    if resize.input_view_in.is_none() {
        return;
    }

    let _ = context.ClearState();
    let _ = context.CSSetShader(resize.pass_h.as_ref(), None);
    let views_in = [resize.input_view_in.clone()];
    let _ = context.CSSetShaderResources(0, Some(&views_in));
    let views_out = [resize.middle_view_out.clone()];
    let _ = context.CSSetUnorderedAccessViews(0, 1, Some(views_out.as_ptr()), None);
    let _ = context.Dispatch(
        (resize.output_width + 15) / 16,
        (resize.input_height + 15) / 16,
        1,
    );

    let _ = context.ClearState();
    let _ = context.CSSetShader(resize.pass_v.as_ref(), None);
    let views_mid = [resize.middle_view_in.clone()];
    let _ = context.CSSetShaderResources(0, Some(&views_mid));
    let views_uv = [resize.output_view_out.clone()];
    let _ = context.CSSetUnorderedAccessViews(0, 1, Some(views_uv.as_ptr()), None);
    let _ = context.Dispatch(
        (resize.output_width + 15) / 16,
        (resize.output_height + 15) / 16,
        1,
    );
}
