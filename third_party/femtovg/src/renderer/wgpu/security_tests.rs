use super::*;
use crate::{Canvas, Color, Paint, Path, RecordingRenderer};

fn many_contours() -> Path {
    let mut path = Path::new();
    for index in 0..128 {
        path.rect(4.0 + (index % 16) as f32 * 16.0, 4.0 + (index / 16) as f32 * 16.0, 8.0, 8.0);
    }
    path
}

fn zero_radius_gradient() -> Paint {
    Paint::radial_gradient(16.0, 16.0, 0.0, 0.0, Color::rgb(255, 0, 0), Color::rgb(0, 0, 255))
}

#[test]
fn security_uniform_staging_reuses_identical_nan_payloads() {
    let mut current = None;
    let mut bytes = Vec::new();
    let mut uniforms = UniformArray::default();
    uniforms.0[0] = f32::from_bits(0x7fc0_0011);
    assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms.clone()), 0);
    for _ in 0..128 { assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms.clone()), 0); }
    assert_eq!(bytes.len(), 256);
    uniforms.0[0] = f32::from_bits(0x7fc0_0012);
    assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms.clone()), 256);
    uniforms.0[0] = 1.0;
    assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms.clone()), 512);
    assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms), 512);
    assert!(bytes.chunks_exact(256).all(|slot| slot[UNIFORM_BYTES as usize..].iter().all(|&byte| byte == 0)));
}

#[test]
fn security_uniform_budget_holds_for_public_many_contour_fill() {
    for (paint, expect_nan) in [(Paint::color(Color::rgb(20, 40, 60)), false), (zero_radius_gradient(), true)] {
        let renderer = RecordingRenderer::default();
        let recorded = renderer.last_commands.clone();
        let mut canvas = Canvas::new(renderer).unwrap();
        canvas.set_size(256, 128, 1.0);
        canvas.fill_path(&many_contours(), &paint);
        canvas.flush_to_output(());
        let commands = recorded.borrow();
        let command = commands.iter().find(|c| matches!(&c.cmd_type, crate::renderer::CommandType::ConcaveFill { .. })).unwrap();
        let crate::renderer::CommandType::ConcaveFill { stencil_params, fill_params } = &command.cmd_type else { unreachable!() };
        assert_eq!(command.drawables.len(), 128);
        assert!(command.drawables.iter().all(|d| d.stroke_verts.is_some()));
        assert_eq!(fill_params.paint_mat.iter().any(|v| v.is_nan()), expect_nan);
        let mut current = None;
        let mut bytes = Vec::new();
        stage_uniforms(&mut current, &mut bytes, 256, UniformArray::from(stencil_params));
        for _ in 0..=command.drawables.len() {
            stage_uniforms(&mut current, &mut bytes, 256, UniformArray::from(fill_params));
        }
        assert!(bytes.len() <= (2 * 256));
        assert!(bytes.len() as u64 <= MIN_UNIFORM_SLOTS.max(commands.len() as u64 * UNIFORM_SLOTS_PER_COMMAND) * 256);
    }
}

#[test]
fn security_uniform_staging_preserves_signed_zero_bits() {
    let mut current = None;
    let mut bytes = Vec::new();
    let mut uniforms = UniformArray::default();
    assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms.clone()), 0);
    uniforms.0[0] = -0.0;
    assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms.clone()), 256);
    assert_eq!(stage_uniforms(&mut current, &mut bytes, 256, uniforms), 256);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "requires a native WGPU adapter"]
fn security_wgpu_many_contour_gradient_has_no_validation_error() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("native WGPU adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("WGPU device");
    let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("security uniform staging output"),
        size: wgpu::Extent3d { width: 256, height: 128, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let renderer = WGPURenderer::new(device.clone(), queue.clone());
    let mut canvas = Canvas::new(renderer).unwrap();
    canvas.set_size(256, 128, 1.0);
    for paint in [Paint::color(Color::rgb(20, 40, 60)), zero_radius_gradient()] {
        canvas.fill_path(&many_contours(), &paint);
        let command = canvas.flush_to_output(&output).expect("draw commands");
        let submission = queue.submit([command]);
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(std::time::Duration::from_secs(10)) })
            .expect("WGPU submission completed");
    }
    let error = pollster::block_on(errors.pop());
    assert!(error.is_none(), "WGPU validation error: {error:?}");
}
