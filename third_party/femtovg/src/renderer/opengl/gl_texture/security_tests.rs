use super::*;
use std::{cell::{Cell, RefCell}, ffi::c_void, num::NonZeroU32};

thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static READ_PIXELS: Cell<bool> = const { Cell::new(false) };
    static CAPTURE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn get_string(name: u32) -> *const u8 {
    if name == glow::VERSION { c"2.1 SecurityTest".as_ptr().cast() } else { c"".as_ptr().cast() }
}
unsafe extern "system" fn bind_texture(_: u32, _: u32) {}
unsafe extern "system" fn pixel_store(_: u32, _: i32) {}
unsafe extern "system" fn upload(_: u32, _: i32, _: i32, _: i32, width: i32, height: i32, format: u32, _: u32, data: *const c_void) {
    CALLS.set(CALLS.get() + 1);
    if READ_PIXELS.get() {
        let bpp = match format { glow::RGB => 3, glow::RGBA => 4, glow::RED | glow::LUMINANCE => 1, _ => panic!("format") };
        let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), width as usize * height as usize * bpp) };
        CAPTURE.with_borrow_mut(|out| *out = bytes.to_vec());
    }
}

fn context() -> Rc<glow::Context> {
    Rc::new(unsafe { glow::Context::from_loader_function(|name| match name {
        "glGetString" => get_string as *const c_void,
        "glBindTexture" => bind_texture as *const c_void,
        "glPixelStorei" => pixel_store as *const c_void,
        "glTexSubImage2D" => upload as *const c_void,
        _ => std::ptr::null(),
    }) })
}

#[test]
fn security_native_upload_rejects_short_data_and_packs_rows() {
    use crate::{imgref::Img, rgb::{RGB8, RGBA8, alt::Gray}};
    let context = context();
    let rgba = [RGBA8::new(1, 2, 3, 4), RGBA8::new(5, 6, 7, 8), RGBA8::new(99, 99, 99, 99), RGBA8::new(9, 10, 11, 12), RGBA8::new(13, 14, 15, 16)];
    let rgb = [RGB8::new(1, 2, 3), RGB8::new(4, 5, 6), RGB8::new(99, 99, 99), RGB8::new(7, 8, 9), RGB8::new(10, 11, 12)];
    let gray = [Gray::new(1), Gray::new(2), Gray::new(99), Gray::new(3), Gray::new(4)];
    let cases = [
        (ImageSource::Rgba(Img::new(&rgba[..1], 2, 2)), ImageSource::Rgba(Img::new_stride(&rgba[..], 2, 2, 3)), (1..=16).collect::<Vec<u8>>()),
        (ImageSource::Rgb(Img::new(&rgb[..1], 2, 2)), ImageSource::Rgb(Img::new_stride(&rgb[..], 2, 2, 3)), (1..=12).collect()),
        (ImageSource::Gray(Img::new(&gray[..1], 2, 2)), ImageSource::Gray(Img::new_stride(&gray[..], 2, 2, 3)), (1..=4).collect()),
    ];
    for es2 in [false, true] {
        for (short, valid, expected) in &cases {
            let info = ImageInfo::new(ImageFlags::empty(), 2, 2, short.format());
            let mut texture = GlTexture::new_from_native_texture(glow::NativeTexture(NonZeroU32::new(1).unwrap()), info, false);
            CALLS.set(0);
            READ_PIXELS.set(false);
            assert!(matches!(texture.update(&context, *short, 0, 0, es2), Err(ErrorKind::InvalidImageData)));
            assert_eq!(CALLS.get(), 0);
            READ_PIXELS.set(true);
            texture.update(&context, *valid, 0, 0, es2).unwrap();
            assert_eq!(CALLS.get(), 1);
            CAPTURE.with_borrow(|bytes| assert_eq!(bytes, expected));
            READ_PIXELS.set(false);
        }
    }
}

#[derive(Default)]
struct LifetimeCalls {
    created_textures: Vec<u32>,
    deleted_textures: Vec<u32>,
    created_framebuffers: Vec<u32>,
    deleted_framebuffers: Vec<u32>,
    created_renderbuffers: Vec<u32>,
    deleted_renderbuffers: Vec<u32>,
    attachments: Vec<u32>,
    renderbuffer_sizes: Vec<(i32, i32)>,
    bound_framebuffer: u32,
    framebuffer_textures: std::collections::HashMap<u32, u32>,
    viewport: [i32; 2],
    view_uniform: [f32; 2],
    clears: Vec<(u32, [i32; 2], [f32; 2])>,
}

thread_local! {
    static LIFETIME_CALLS: RefCell<LifetimeCalls> = RefCell::new(LifetimeCalls::default());
}

unsafe extern "system" fn gen_textures(count: i32, textures: *mut u32) {
    assert_eq!(count, 1);
    LIFETIME_CALLS.with_borrow_mut(|calls| {
        let id = calls.created_textures.len() as u32 + 1;
        calls.created_textures.push(id);
        unsafe { textures.write(id) };
    });
}

unsafe extern "system" fn delete_textures(count: i32, textures: *const u32) {
    assert_eq!(count, 1);
    LIFETIME_CALLS.with_borrow_mut(|calls| calls.deleted_textures.push(unsafe { textures.read() }));
}

unsafe extern "system" fn allocate_texture(_: u32, _: i32, _: i32, _: i32, _: i32, _: i32, _: u32, _: u32, _: *const c_void) {}
unsafe extern "system" fn texture_parameter(_: u32, _: u32, _: i32) {}

unsafe extern "system" fn gen_framebuffers(count: i32, framebuffers: *mut u32) {
    assert_eq!(count, 1);
    LIFETIME_CALLS.with_borrow_mut(|calls| {
        let id = calls.created_framebuffers.len() as u32 + 1;
        calls.created_framebuffers.push(id);
        unsafe { framebuffers.write(id) };
    });
}

unsafe extern "system" fn delete_framebuffers(count: i32, framebuffers: *const u32) {
    assert_eq!(count, 1);
    LIFETIME_CALLS.with_borrow_mut(|calls| {
        let framebuffer = unsafe { framebuffers.read() };
        calls.deleted_framebuffers.push(framebuffer);
        calls.framebuffer_textures.remove(&framebuffer);
        if calls.bound_framebuffer == framebuffer {
            calls.bound_framebuffer = 0;
        }
    });
}

unsafe extern "system" fn bind_framebuffer(_: u32, framebuffer: u32) {
    LIFETIME_CALLS.with_borrow_mut(|calls| calls.bound_framebuffer = framebuffer);
}
unsafe extern "system" fn attach_texture(_: u32, _: u32, _: u32, texture: u32, _: i32) {
    LIFETIME_CALLS.with_borrow_mut(|calls| {
        calls.attachments.push(texture);
        calls.framebuffer_textures.insert(calls.bound_framebuffer, texture);
    });
}

unsafe extern "system" fn gen_renderbuffers(count: i32, renderbuffers: *mut u32) {
    assert_eq!(count, 1);
    LIFETIME_CALLS.with_borrow_mut(|calls| {
        let id = calls.created_renderbuffers.len() as u32 + 1;
        calls.created_renderbuffers.push(id);
        unsafe { renderbuffers.write(id) };
    });
}

unsafe extern "system" fn delete_renderbuffers(count: i32, renderbuffers: *const u32) {
    assert_eq!(count, 1);
    LIFETIME_CALLS.with_borrow_mut(|calls| calls.deleted_renderbuffers.push(unsafe { renderbuffers.read() }));
}

unsafe extern "system" fn bind_renderbuffer(_: u32, _: u32) {}
unsafe extern "system" fn renderbuffer_storage(_: u32, _: u32, width: i32, height: i32) {
    LIFETIME_CALLS.with_borrow_mut(|calls| calls.renderbuffer_sizes.push((width, height)));
}
unsafe extern "system" fn attach_renderbuffer(_: u32, _: u32, _: u32, _: u32) {}
unsafe extern "system" fn framebuffer_status(_: u32) -> u32 { glow::FRAMEBUFFER_COMPLETE }
unsafe extern "system" fn viewport(_: i32, _: i32, width: i32, height: i32) {
    LIFETIME_CALLS.with_borrow_mut(|calls| calls.viewport = [width, height]);
}

unsafe extern "system" fn create_shader(_: u32) -> u32 { 1 }
unsafe extern "system" fn shader_source(_: u32, _: i32, _: *const *const i8, _: *const i32) {}
unsafe extern "system" fn object_action(_: u32) {}
unsafe extern "system" fn object_status(_: u32, _: u32, status: *mut i32) { unsafe { status.write(1) }; }
unsafe extern "system" fn create_program() -> u32 { 1 }
unsafe extern "system" fn program_shader(_: u32, _: u32) {}
unsafe extern "system" fn attribute_location(_: u32, _: u32, _: *const i8) {}
unsafe extern "system" fn uniform_location(_: u32, _: *const i8) -> i32 { 1 }
unsafe extern "system" fn view_uniform(_: i32, _: i32, values: *const f32) {
    LIFETIME_CALLS.with_borrow_mut(|calls| calls.view_uniform = unsafe { [values.read(), values.add(1).read()] });
}
unsafe extern "system" fn color_mask(_: u8, _: u8, _: u8, _: u8) {}
unsafe extern "system" fn stencil_op(_: u32, _: u32, _: u32) {}
unsafe extern "system" fn stencil_func(_: u32, _: i32, _: u32) {}
unsafe extern "system" fn bind_buffer(_: u32, _: u32) {}
unsafe extern "system" fn buffer_data(_: u32, _: isize, _: *const c_void, _: u32) {}
unsafe extern "system" fn attribute_pointer(_: u32, _: i32, _: u32, _: u8, _: i32, _: *const c_void) {}
unsafe extern "system" fn blend_func(_: u32, _: u32, _: u32, _: u32) {}
unsafe extern "system" fn scissor(_: i32, _: i32, _: i32, _: i32) {}
unsafe extern "system" fn clear_color(_: f32, _: f32, _: f32, _: f32) {}
unsafe extern "system" fn clear(_: u32) {
    LIFETIME_CALLS.with_borrow_mut(|calls| {
        let texture = calls.framebuffer_textures.get(&calls.bound_framebuffer).copied().unwrap_or(0);
        calls.clears.push((texture, calls.viewport, calls.view_uniform));
    });
}

fn lifetime_renderer() -> super::super::OpenGl {
    LIFETIME_CALLS.with_borrow_mut(|calls| *calls = LifetimeCalls::default());
    let context = Rc::new(unsafe { glow::Context::from_loader_function(|name| match name {
        "glGetString" => get_string as *const c_void,
        "glBindTexture" => bind_texture as *const c_void,
        "glPixelStorei" => pixel_store as *const c_void,
        "glGenTextures" => gen_textures as *const c_void,
        "glDeleteTextures" => delete_textures as *const c_void,
        "glTexImage2D" => allocate_texture as *const c_void,
        "glTexParameteri" => texture_parameter as *const c_void,
        "glGenFramebuffers" => gen_framebuffers as *const c_void,
        "glDeleteFramebuffers" => delete_framebuffers as *const c_void,
        "glBindFramebuffer" => bind_framebuffer as *const c_void,
        "glFramebufferTexture2D" => attach_texture as *const c_void,
        "glGenRenderbuffers" => gen_renderbuffers as *const c_void,
        "glDeleteRenderbuffers" => delete_renderbuffers as *const c_void,
        "glBindRenderbuffer" => bind_renderbuffer as *const c_void,
        "glRenderbufferStorage" => renderbuffer_storage as *const c_void,
        "glFramebufferRenderbuffer" => attach_renderbuffer as *const c_void,
        "glCheckFramebufferStatus" => framebuffer_status as *const c_void,
        "glViewport" => viewport as *const c_void,
        "glCreateShader" => create_shader as *const c_void,
        "glShaderSource" => shader_source as *const c_void,
        "glCompileShader" | "glLinkProgram" | "glDeleteShader" | "glDeleteProgram" | "glUseProgram"
            | "glEnable" | "glDisable" | "glCullFace" | "glFrontFace" | "glStencilMask" | "glActiveTexture"
            | "glBindVertexArray" | "glEnableVertexAttribArray" | "glDisableVertexAttribArray" => object_action as *const c_void,
        "glGetShaderiv" | "glGetProgramiv" => object_status as *const c_void,
        "glCreateProgram" => create_program as *const c_void,
        "glAttachShader" | "glDetachShader" => program_shader as *const c_void,
        "glBindAttribLocation" => attribute_location as *const c_void,
        "glGetUniformLocation" => uniform_location as *const c_void,
        "glUniform2fv" => view_uniform as *const c_void,
        "glColorMask" => color_mask as *const c_void,
        "glStencilOp" => stencil_op as *const c_void,
        "glStencilFunc" => stencil_func as *const c_void,
        "glBindBuffer" => bind_buffer as *const c_void,
        "glBufferData" => buffer_data as *const c_void,
        "glVertexAttribPointer" => attribute_pointer as *const c_void,
        "glBlendFuncSeparate" => blend_func as *const c_void,
        "glScissor" => scissor as *const c_void,
        "glClearColor" => clear_color as *const c_void,
        "glClear" => clear as *const c_void,
        _ => std::ptr::null(),
    }) });

    super::super::OpenGl {
        debug: false,
        antialias: false,
        is_opengles_2_0: false,
        view: [0.0; 2],
        screen_view: [0.0; 2],
        main_programs_with_glyph_texture: std::array::from_fn(|_| None),
        main_programs_without_glyph_texture: std::array::from_fn(|_| None),
        current_program: 0,
        current_program_needs_glyph_texture: false,
        vert_arr: None,
        vert_buff: None,
        framebuffers: Default::default(),
        context,
        screen_target: None,
        current_render_target: crate::RenderTarget::Screen,
    }
}

fn rendering_lifetime_renderer() -> super::super::OpenGl {
    let mut renderer = lifetime_renderer();
    renderer.main_programs_without_glyph_texture[0] = Some(super::super::MainProgram::new(
        &renderer.context,
        false,
        crate::renderer::ShaderType::FillGradient,
        false,
    ).unwrap());
    renderer
}

#[test]
fn security_realloc_releases_gl_resources_and_rebuilds_framebuffers() {
    let mut renderer = lifetime_renderer();
    let mut images = crate::ImageStore::new();
    let info = ImageInfo::new(ImageFlags::empty(), 2, 2, PixelFormat::Rgba8);
    let id = images.alloc(&mut renderer, info).unwrap();
    renderer.set_target(&images, crate::RenderTarget::Image(id));

    for width in 3..=5 {
        let info = ImageInfo::new(ImageFlags::empty(), width, 4, PixelFormat::Rgba8);
        images.realloc(&mut renderer, id, info).unwrap();
        assert!(!renderer.framebuffers.contains_key(&id));
        LIFETIME_CALLS.with_borrow(|calls| {
            assert_eq!(calls.created_textures.len(), calls.deleted_textures.len() + 1);
            assert_eq!(calls.created_framebuffers, calls.deleted_framebuffers);
            assert_eq!(calls.created_renderbuffers, calls.deleted_renderbuffers);
        });

        renderer.set_target(&images, crate::RenderTarget::Image(id));
        LIFETIME_CALLS.with_borrow(|calls| {
            assert_eq!(calls.attachments.last(), calls.created_textures.last());
            assert_eq!(calls.renderbuffer_sizes.last(), Some(&(width as i32, 4)));
        });
    }

    images.remove(&mut renderer, id);
    assert!(renderer.framebuffers.is_empty());
    LIFETIME_CALLS.with_borrow(|calls| {
        assert_eq!(calls.created_textures, calls.deleted_textures);
        assert_eq!(calls.created_framebuffers, calls.deleted_framebuffers);
        assert_eq!(calls.created_renderbuffers, calls.deleted_renderbuffers);
    });
}

#[test]
fn security_realloc_preserves_borrowed_gl_textures_and_removes_their_framebuffers() {
    for external in [false, true] {
        let mut renderer = lifetime_renderer();
        let mut images = crate::ImageStore::new();
        let info = ImageInfo::new(ImageFlags::empty(), 2, 2, PixelFormat::Rgba8);
        let texture = glow::NativeTexture(NonZeroU32::new(999).unwrap());
        let id = if external {
            images.register_external_texture(&mut renderer, texture, info).unwrap()
        } else {
            images.register_native_texture(&mut renderer, texture, info).unwrap()
        };
        renderer.set_target(&images, crate::RenderTarget::Image(id));

        images.realloc(&mut renderer, id, info).unwrap();
        assert!(!renderer.framebuffers.contains_key(&id));
        LIFETIME_CALLS.with_borrow(|calls| {
            assert!(calls.deleted_textures.is_empty());
            assert_eq!(calls.created_framebuffers, calls.deleted_framebuffers);
            assert_eq!(calls.created_renderbuffers, calls.deleted_renderbuffers);
        });

        renderer.set_target(&images, crate::RenderTarget::Image(id));
        LIFETIME_CALLS.with_borrow(|calls| assert_eq!(calls.attachments, [999, 1]));
        images.clear(&mut renderer);
        LIFETIME_CALLS.with_borrow(|calls| {
            assert_eq!(calls.deleted_textures, [1]);
            assert_eq!(calls.created_framebuffers, calls.deleted_framebuffers);
            assert_eq!(calls.created_renderbuffers, calls.deleted_renderbuffers);
        });
    }
}

#[test]
fn security_realloc_preserves_active_canvas_target_with_same_id_selection() {
    let mut canvas = crate::Canvas::new(rendering_lifetime_renderer()).unwrap();
    canvas.set_size(100, 100, 1.0);
    let id = canvas.create_image_empty(2, 3, PixelFormat::Rgba8, ImageFlags::empty()).unwrap();
    canvas.set_render_target(crate::RenderTarget::Image(id));
    canvas.clear_rect(0, 0, 1, 1, crate::Color::white());
    canvas.flush_to_output(());

    canvas.realloc_image(id, 4, 5, PixelFormat::Rgba8, ImageFlags::empty()).unwrap();
    canvas.set_render_target(crate::RenderTarget::Image(id));
    canvas.clear_rect(0, 0, 1, 1, crate::Color::white());
    canvas.flush_to_output(());

    LIFETIME_CALLS.with_borrow(|calls| {
        assert_eq!(calls.clears, [(1, [2, 3], [2.0, 3.0]), (2, [4, 5], [4.0, 5.0])]);
    });
}

#[test]
fn security_realloc_preserves_queued_canvas_draws_and_target_changes() {
    let mut canvas = crate::Canvas::new(rendering_lifetime_renderer()).unwrap();
    canvas.set_size(100, 100, 1.0);
    let id = canvas.create_image_empty(2, 3, PixelFormat::Rgba8, ImageFlags::empty()).unwrap();
    canvas.set_render_target(crate::RenderTarget::Image(id));
    canvas.clear_rect(0, 0, 1, 1, crate::Color::white());
    canvas.flush_to_output(());

    canvas.clear_rect(0, 0, 1, 1, crate::Color::white());
    canvas.set_render_target(crate::RenderTarget::Screen);
    canvas.clear_rect(0, 0, 1, 1, crate::Color::white());
    canvas.realloc_image(id, 4, 5, PixelFormat::Rgba8, ImageFlags::empty()).unwrap();
    canvas.set_render_target(crate::RenderTarget::Image(id));
    canvas.clear_rect(0, 0, 1, 1, crate::Color::white());
    canvas.flush_to_output(());

    LIFETIME_CALLS.with_borrow(|calls| {
        assert_eq!(calls.clears, [
            (1, [2, 3], [2.0, 3.0]),
            (2, [4, 5], [4.0, 5.0]),
            (0, [100, 100], [100.0, 100.0]),
            (2, [4, 5], [4.0, 5.0]),
        ]);
    });
}
