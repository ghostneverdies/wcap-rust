use windows::Win32::Graphics::Direct3D11::*;

use crate::shaders::*;
use crate::wcap::*;

use windows::Win32::Graphics::{
    Direct3D::{Fxc::D3DDecompressShaders, ID3DBlob, D3D_SRV_DIMENSION_TEXTURE2D},
    Dxgi::Common::*,
};

pub unsafe fn yuv_convert_output_create(
    output: &mut YuvConvertOutput,
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) {
    debug_assert!(width % 2 == 0 && height % 2 == 0);
    debug_assert!(format == DXGI_FORMAT_NV12 || format == DXGI_FORMAT_P010);
    *output = YuvConvertOutput::default();

    let mut texture_desc = D3D11_TEXTURE2D_DESC::default();
    texture_desc.Width = width;
    texture_desc.Height = height;
    texture_desc.MipLevels = 1;
    texture_desc.ArraySize = 1;
    texture_desc.Format = format;
    texture_desc.SampleDesc = DXGI_SAMPLE_DESC {
        Count: 1,
        Quality: 0,
    };
    texture_desc.Usage = D3D11_USAGE_DEFAULT;
    texture_desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS).0 as u32;

    device
        .CreateTexture2D(&texture_desc, None, Some(&mut output.texture))
        .expect("CreateTexture2D failed");

    let mut view_out_desc_y = D3D11_UNORDERED_ACCESS_VIEW_DESC::default();
    view_out_desc_y.Format = if format == DXGI_FORMAT_NV12 {
        DXGI_FORMAT_R8_UNORM
    } else {
        DXGI_FORMAT_R16_UNORM
    };
    view_out_desc_y.ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;

    let mut view_out_desc_uv = D3D11_UNORDERED_ACCESS_VIEW_DESC::default();
    view_out_desc_uv.Format = if format == DXGI_FORMAT_NV12 {
        DXGI_FORMAT_R8G8_UNORM
    } else {
        DXGI_FORMAT_R16G16_UNORM
    };
    view_out_desc_uv.ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;

    let mut view_in_desc_uv = D3D11_SHADER_RESOURCE_VIEW_DESC::default();
    view_in_desc_uv.Format = if format == DXGI_FORMAT_NV12 {
        DXGI_FORMAT_R8G8_UNORM
    } else {
        DXGI_FORMAT_R16G16_UNORM
    };
    view_in_desc_uv.ViewDimension = D3D_SRV_DIMENSION_TEXTURE2D;
    view_in_desc_uv.Anonymous.Texture2D.MipLevels = 0xFFFFFFFF;

    device
        .CreateUnorderedAccessView(
            output.texture.as_ref().unwrap(),
            Some(&view_out_desc_y as *const D3D11_UNORDERED_ACCESS_VIEW_DESC),
            Some(&mut output.view_out_y),
        )
        .expect("CreateUnorderedAccessView failed");
    device
        .CreateUnorderedAccessView(
            output.texture.as_ref().unwrap(),
            Some(&view_out_desc_uv as *const D3D11_UNORDERED_ACCESS_VIEW_DESC),
            Some(&mut output.view_out_uv),
        )
        .expect("CreateUnorderedAccessView failed");
    device
        .CreateShaderResourceView(
            output.texture.as_ref().unwrap(),
            Some(&view_in_desc_uv as *const D3D11_SHADER_RESOURCE_VIEW_DESC),
            Some(&mut output.view_in_uv),
        )
        .expect("CreateShaderResourceView failed");
}

pub unsafe fn yuv_convert_output_release(output: &mut YuvConvertOutput) {
    *output = YuvConvertOutput::default();
}

pub unsafe fn yuv_convert_create(
    convert: &mut YuvConvert,
    device: &ID3D11Device,
    input_texture: &ID3D11Texture2D,
    width: u32,
    height: u32,
    color_space: u32,
    improved_conversion: bool,
) {
    debug_assert!(width % 2 == 0 && height % 2 == 0);
    *convert = YuvConvert::default();

    let mut input_view_desc = D3D11_SHADER_RESOURCE_VIEW_DESC::default();
    input_view_desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    input_view_desc.ViewDimension = D3D_SRV_DIMENSION_TEXTURE2D;
    input_view_desc.Anonymous.Texture2D.MipLevels = 0xFFFFFFFF;

    device
        .CreateShaderResourceView(
            input_texture,
            Some(&input_view_desc as *const D3D11_SHADER_RESOURCE_VIEW_DESC),
            Some(&mut convert.input_view),
        )
        .expect("CreateShaderResourceView failed");

    static BT2020: [[f32; 4]; 6] = [
        [0.26270f32, 0.678000f32, 0.0593000f32, 0.0],
        [-0.13963f32, -0.360370f32, 0.5000000f32, 0.0],
        [0.50000f32, -0.459786f32, -0.0402143f32, 0.0],
        [1.0f32, 0.000000f32, 1.474600f32, 0.0],
        [1.0f32, -0.164553f32, -0.571353f32, 0.0],
        [1.0f32, 1.881400f32, 0.000000f32, 0.0],
    ];

    static BT709: [[f32; 4]; 6] = [
        [0.2126f32, 0.7152f32, 0.0722f32, 0.0],
        [-0.1146f32, -0.3854f32, 0.5000f32, 0.0],
        [0.50000f32, -0.4542f32, -0.0458f32, 0.0],
        [1.0f32, 0.000000f32, 1.5748f32, 0.0],
        [1.0f32, -0.1873f32, -0.4681f32, 0.0],
        [1.0f32, 1.8556f32, 0.000000f32, 0.0],
    ];

    static BT601: [[f32; 4]; 6] = [
        [0.299000f32, 0.587000f32, 0.114000f32, 0.0],
        [-0.168736f32, -0.331264f32, 0.500000f32, 0.0],
        [0.500000f32, -0.418688f32, -0.081312f32, 0.0],
        [1.0f32, 0.000000f32, 1.402000f32, 0.0],
        [1.0f32, -0.344136f32, -0.714136f32, 0.0],
        [1.0f32, 1.772000f32, 0.000000f32, 0.0],
    ];

    let convert_matrix = match color_space {
        YUV_COLOR_SPACE_BT2020 => &BT2020,
        YUV_COLOR_SPACE_BT709 => &BT709,
        YUV_COLOR_SPACE_BT601 => &BT601,
        _ => &BT601,
    };

    let mut constant_buffer_desc = D3D11_BUFFER_DESC::default();
    constant_buffer_desc.ByteWidth = 6 * 4 * std::mem::size_of::<f32>() as u32;
    constant_buffer_desc.Usage = D3D11_USAGE_IMMUTABLE;
    constant_buffer_desc.BindFlags = D3D11_BIND_CONSTANT_BUFFER.0 as u32;

    let mut constant_buffer_data = D3D11_SUBRESOURCE_DATA::default();
    constant_buffer_data.pSysMem = convert_matrix as *const _ as *const std::ffi::c_void;

    device
        .CreateBuffer(
            &constant_buffer_desc,
            Some(&constant_buffer_data),
            Some(&mut convert.constant_buffer),
        )
        .expect("CreateBuffer failed");

    if improved_conversion {
        let mut shader: Option<ID3DBlob> = None;
        D3DDecompressShaders(
            CONVERT_PASS1_SHADER_BYTES.as_ptr() as _,
            CONVERT_PASS1_SHADER_BYTES.len(),
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
                .CreateComputeShader(shader_bytes, None, Some(&mut convert.pass1))
                .expect("CreateComputeShader failed");
            drop(shader);
        }

        let mut shader: Option<ID3DBlob> = None;
        D3DDecompressShaders(
            CONVERT_PASS2_SHADER_BYTES.as_ptr() as _,
            CONVERT_PASS2_SHADER_BYTES.len(),
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
                .CreateComputeShader(shader_bytes, None, Some(&mut convert.pass2))
                .expect("CreateComputeShader failed");
            drop(shader);
        }

        convert.single_pass = None;
    } else {
        let mut shader: Option<ID3DBlob> = None;
        D3DDecompressShaders(
            CONVERT_SINGLE_PASS_SHADER_BYTES.as_ptr() as _,
            CONVERT_SINGLE_PASS_SHADER_BYTES.len(),
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
                .CreateComputeShader(shader_bytes, None, Some(&mut convert.single_pass))
                .expect("CreateComputeShader failed");
            drop(shader);
        }
    }

    convert.width = width;
    convert.height = height;
}

pub unsafe fn yuv_convert_release(convert: &mut YuvConvert) {
    *convert = YuvConvert::default();
}

pub unsafe fn yuv_convert_dispatch(
    convert: &mut YuvConvert,
    context: &ID3D11DeviceContext,
    output: &mut YuvConvertOutput,
) {
    if convert.single_pass.is_some() {
        let output_views = [output.view_out_y.clone(), output.view_out_uv.clone()];

        let _ = context.ClearState();
        let _ = context.CSSetShader(convert.single_pass.as_ref(), None);
        let constant_buffers = [convert.constant_buffer.clone()];
        let _ = context.CSSetConstantBuffers(0, Some(&constant_buffers));
        let input_views = [convert.input_view.clone()];
        let _ = context.CSSetShaderResources(0, Some(&input_views));
        let _ = context.CSSetUnorderedAccessViews(0, 2, Some(output_views.as_ptr()), None);
        let _ = context.Dispatch(
            (convert.width / 2 + 15) / 16,
            (convert.height / 2 + 15) / 16,
            1,
        );
    } else {
        let _ = context.ClearState();
        let _ = context.CSSetShader(convert.pass1.as_ref(), None);
        let constant_buffers = [convert.constant_buffer.clone()];
        let _ = context.CSSetConstantBuffers(0, Some(&constant_buffers));
        let input_views = [convert.input_view.clone()];
        let _ = context.CSSetShaderResources(0, Some(&input_views));
        let output_views_uv = [output.view_out_uv.clone()];
        let _ = context.CSSetUnorderedAccessViews(1, 1, Some(output_views_uv.as_ptr()), None);
        let _ = context.Dispatch(
            (convert.width / 2 + 15) / 16,
            (convert.height / 2 + 15) / 16,
            1,
        );

        let input_views = [convert.input_view.clone(), output.view_in_uv.clone()];

        let _ = context.ClearState();
        let _ = context.CSSetShader(convert.pass2.as_ref(), None);
        let constant_buffers = [convert.constant_buffer.clone()];
        let _ = context.CSSetConstantBuffers(0, Some(&constant_buffers));
        let _ = context.CSSetShaderResources(0, Some(&input_views));
        let output_views_y = [output.view_out_y.clone()];
        let _ = context.CSSetUnorderedAccessViews(0, 1, Some(output_views_y.as_ptr()), None);
        let _ = context.Dispatch((convert.width + 15) / 16, (convert.height + 15) / 16, 1);
    }
}
