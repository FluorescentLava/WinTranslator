//! Native animation ABI below is verified against Windows App Runtime 2.4
//! Microsoft.UI.winmd and Microsoft.UI.Xaml.winmd. Indices exclude IInspectable's six slots.
use super::*;
use crate::reference::SlideYAnimation;
use ::core;
use std::ffi::c_void;
use windows_core::{HRESULT, HSTRING, IInspectable, IInspectable_Vtbl, IUnknown};

type Raw = *mut c_void;
type NoArgs = unsafe extern "system" fn(Raw) -> HRESULT;
type StringArg = unsafe extern "system" fn(Raw, Raw) -> HRESULT;
type GetObject = unsafe extern "system" fn(Raw, *mut Raw) -> HRESULT;
windows_core::imp::define_interface!(
    SlideObject,
    IInspectable_Vtbl,
    0x0e583d49_fb5e_5481_a426_d3c41e059a5a
);
windows_core::imp::define_interface!(
    SlideVisual,
    IInspectable_Vtbl,
    0xc0eeab6c_c897_5ac6_a1c9_63abd5055b9b
);
windows_core::imp::define_interface!(
    KeyFrames,
    IInspectable_Vtbl,
    0x5a8f57f0_f059_5b47_b308_c4c80fc71248
);
windows_core::imp::define_interface!(
    ScalarFrames,
    IInspectable_Vtbl,
    0x5a5f8abe_d129_5b25_8aff_8180fd9bfb22
);
windows_core::imp::define_interface!(
    Easing,
    IInspectable_Vtbl,
    0x8e1ecd0d_57d8_5bc9_9bcd_e43d0dd733c4
);
windows_core::imp::define_interface!(
    Animation,
    IInspectable_Vtbl,
    0xa829ccc8_6fde_5b90_ad37_efd307e1b631
);
windows_core::imp::define_interface!(
    Batch,
    IInspectable_Vtbl,
    0xd31ca572_99ce_5969_b042_6c2d330a3859
);
windows_core::imp::define_interface!(
    PropertySet,
    IInspectable_Vtbl,
    0x97f7a17b_97be_5545_9f1c_0b9d44577f57
);
windows_core::imp::define_interface!(
    BatchArgs,
    IInspectable_Vtbl,
    0xac400334_4358_5fb0_bfc3_117fe581998f
);
impl windows_core::RuntimeType for BatchArgs {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(
        b"rc(Microsoft.UI.Composition.CompositionBatchCompletedEventArgs;{ac400334-4358-5fb0-bfc3-117fe581998f})");
}

// All calls use verified public WinRT ABI, with QueryInterface selecting the interface first.
unsafe fn method<T: Copy>(object: &impl Interface, index: usize) -> T {
    unsafe {
        let table = *(object.as_raw() as *const *const usize);
        std::mem::transmute_copy(&*table.add(6 + index))
    }
}
fn string_raw(value: &HSTRING) -> Raw {
    unsafe { std::mem::transmute_copy(value) }
}
fn get_object(object: &impl Interface, index: usize) -> windows_core::Result<IUnknown> {
    unsafe {
        let mut result = std::ptr::null_mut();
        method::<GetObject>(object, index)(object.as_raw(), &mut result).ok()?;
        Ok(IUnknown::from_raw(result))
    }
}
fn stop(object: &SlideObject, property: &str) {
    let property = HSTRING::from(property);
    unsafe {
        _ = method::<StringArg>(object, 3)(object.as_raw(), string_raw(&property));
    }
}
fn start(object: &SlideObject, property: &str, animation: &IUnknown) -> windows_core::Result<()> {
    let animation = animation.cast::<Animation>()?;
    let property = HSTRING::from(property);
    unsafe {
        method::<unsafe extern "system" fn(Raw, Raw, Raw) -> HRESULT>(object, 2)(
            object.as_raw(),
            string_raw(&property),
            animation.as_raw(),
        )
        .ok()
    }
}
fn easing(compositor: &Compositor, entering: bool) -> windows_core::Result<Easing> {
    // Fluent entrance/departure curves, evaluated by native Composition.
    let (first, second) = if entering {
        ((0.1, 0.9), (0.2, 1.0))
    } else {
        ((0.7, 0.0), (1.0, 0.5))
    };
    let mut result = std::ptr::null_mut();
    unsafe {
        method::<
            unsafe extern "system" fn(
                Raw,
                windows_numerics::Vector2,
                windows_numerics::Vector2,
                *mut Raw,
            ) -> HRESULT,
        >(compositor, 4)(
            compositor.as_raw(),
            windows_numerics::Vector2 {
                x: first.0,
                y: first.1,
            },
            windows_numerics::Vector2 {
                x: second.0,
                y: second.1,
            },
            &mut result,
        )
        .ok()?;
        IUnknown::from_raw(result).cast()
    }
}
fn scalar(
    compositor: &Compositor,
    to: f32,
    ms: u32,
    easing: &Easing,
) -> windows_core::Result<IUnknown> {
    let animation = get_object(compositor, 14)?;
    let frames = animation.cast::<KeyFrames>()?;
    let scalar = animation.cast::<ScalarFrames>()?;
    let expression = HSTRING::from("this.StartingValue");
    unsafe {
        method::<unsafe extern "system" fn(Raw, i64) -> HRESULT>(&frames, 3)(
            frames.as_raw(),
            i64::from(ms.max(1)) * 10_000,
        )
        .ok()?;
        // AnimationStopBehavior.LeaveCurrentValue = 0.
        method::<unsafe extern "system" fn(Raw, i32) -> HRESULT>(&frames, 10)(frames.as_raw(), 0)
            .ok()?;
        method::<unsafe extern "system" fn(Raw, f32, Raw) -> HRESULT>(&frames, 11)(
            frames.as_raw(),
            0.0,
            string_raw(&expression),
        )
        .ok()?;
        method::<unsafe extern "system" fn(Raw, f32, f32, Raw) -> HRESULT>(&scalar, 1)(
            scalar.as_raw(),
            1.0,
            to,
            easing.as_raw(),
        )
        .ok()?;
    }
    Ok(animation)
}
fn translation(properties: &PropertySet, y: f32) -> windows_core::Result<()> {
    let name = HSTRING::from("Translation");
    unsafe {
        method::<unsafe extern "system" fn(Raw, Raw, windows_numerics::Vector3) -> HRESULT>(
            properties, 6,
        )(
            properties.as_raw(),
            string_raw(&name),
            windows_numerics::Vector3 { x: 0.0, y, z: 0.0 },
        )
        .ok()
    }
}
fn opacity(visual: &SlideVisual, value: f32) -> windows_core::Result<()> {
    unsafe {
        method::<unsafe extern "system" fn(Raw, f32) -> HRESULT>(visual, 17)(visual.as_raw(), value)
            .ok()
    }
}

fn settle(element: &UIElement, y: f32, alpha: f32) {
    // If native keyframe construction fails, leave a usable endpoint instead
    // of a transparent/offscreen card. Its completion still reports the error.
    _ = element.SetOpacity(f64::from(alpha));
    if let Ok(visual) = ElementCompositionPreview::GetElementVisual(element) {
        if let Ok(object) = visual.cast::<SlideObject>() {
            stop(&object, "Opacity");
            if let Ok(properties) = get_object(&object, 1).and_then(|v| v.cast::<PropertySet>()) {
                if let Ok(translated) = properties.cast::<SlideObject>() {
                    stop(&translated, "Translation.Y");
                }
                _ = translation(&properties, y);
            }
        }
        if let Ok(visual) = visual.cast::<SlideVisual>() {
            _ = opacity(&visual, alpha);
        }
    }
}

pub(super) struct SlideState {
    revision: u64,
    translation: SlideObject,
    visual: SlideObject,
    completion: Callback<Result<bool, RuntimeError>>,
    _batch: Batch,
    _completed: windows_core::EventRevoker,
}
pub(super) fn cancel(
    states: &Rc<RefCell<HashMap<NodeId, SlideState>>>,
    node: NodeId,
    result: Result<bool, RuntimeError>,
    sink: &EventSink,
) -> bool {
    let previous = states.borrow_mut().remove(&node);
    if let Some(previous) = previous {
        stop(&previous.translation, "Translation.Y");
        stop(&previous.visual, "Opacity");
        sink.enqueue_host(HostEvent::ElementSlideCompleted {
            node,
            completion: previous.completion.clone(),
            result,
        });
        true
    } else {
        false
    }
}
pub(super) fn request(
    states: &Rc<RefCell<HashMap<NodeId, SlideState>>>,
    node: NodeId,
    revision: u64,
    element: &UIElement,
    animation: Option<SlideYAnimation>,
    completion: Callback<Result<bool, RuntimeError>>,
    sink: EventSink,
) -> Result<(), RuntimeError> {
    let continuing = cancel(states, node, Ok(false), &sink);
    let Some(animation) = animation else {
        sink.enqueue_host(HostEvent::ElementSlideCompleted {
            node,
            completion,
            result: Ok(continuing),
        });
        return Ok(());
    };
    if ![
        animation.from_y,
        animation.to_y,
        animation.from_opacity,
        animation.to_opacity,
    ]
    .into_iter()
    .all(f32::is_finite)
    {
        return Err(RuntimeError::Native(0x80070057u32 as i32));
    }
    let result =
        (|| -> windows_core::Result<()> {
            static PREVIEW: windows_core::imp::FactoryCache<
                ElementCompositionPreview,
                IElementCompositionPreviewStatics,
            > = windows_core::imp::FactoryCache::new();
            PREVIEW.call(|factory| unsafe {
                method::<unsafe extern "system" fn(Raw, Raw, bool) -> HRESULT>(factory, 6)(
                    factory.as_raw(),
                    element.as_raw(),
                    true,
                )
                .ok()
            })?;
            let visual = ElementCompositionPreview::GetElementVisual(element)?;
            let object = visual.cast::<SlideObject>()?;
            let properties = get_object(&object, 1)?.cast::<PropertySet>()?;
            let translated = properties.cast::<SlideObject>()?;
            let compositor = visual.cast::<ICompositionObject>()?.Compositor()?;
            let visual_properties = visual.cast::<SlideVisual>()?;
            // Remove any completed animation attachment before preparing another presentation.
            stop(&translated, "Translation.Y");
            stop(&object, "Opacity");
            // Prepare synchronously in the same dispatcher turn as StartAnimation, before rendering.
            element.SetOpacity(1.0)?;
            if !continuing {
                translation(&properties, animation.from_y)?;
                opacity(&visual_properties, animation.from_opacity.clamp(0.0, 1.0))?;
            }
            let easing = easing(&compositor, animation.to_y < animation.from_y)?;
            let movement = scalar(&compositor, animation.to_y, animation.duration_ms, &easing)?;
            let fading = scalar(
                &compositor,
                animation.to_opacity.clamp(0.0, 1.0),
                animation.duration_ms,
                &easing,
            )?;
            let mut batch_raw = std::ptr::null_mut();
            unsafe {
                method::<unsafe extern "system" fn(Raw,i32,*mut Raw)->HRESULT>(&compositor,15)(
            compositor.as_raw(),1,&mut batch_raw).ok()?;
            }
            let batch = unsafe { IUnknown::from_raw(batch_raw) }.cast::<Batch>()?;
            let weak = Rc::downgrade(states);
            let finished_sink = sink.clone();
            let handler = TypedEventHandler::<IInspectable, BatchArgs>::new(move |_, _| {
                let Some(states) = weak.upgrade() else {
                    return;
                };
                let state = {
                    let mut states = states.borrow_mut();
                    if states.get(&node).is_some_and(|v| v.revision == revision) {
                        states.remove(&node)
                    } else {
                        None
                    }
                };
                if let Some(state) = state {
                    finished_sink.enqueue_host(HostEvent::ElementSlideCompleted {
                        node,
                        completion: state.completion.clone(),
                        result: Ok(true),
                    });
                }
            });
            let mut token = 0;
            let revoker = unsafe {
                method::<unsafe extern "system" fn(Raw, Raw, *mut i64) -> HRESULT>(&batch, 5)(
                    batch.as_raw(),
                    handler.as_raw(),
                    &mut token,
                )
                .ok()?;
                windows_core::EventRevoker::new(
                    batch.clone(),
                    token,
                    method::<unsafe extern "system" fn(Raw, i64) -> HRESULT>(&batch, 6),
                )
            };
            states.borrow_mut().insert(
                node,
                SlideState {
                    revision,
                    translation: translated.clone(),
                    visual: object.clone(),
                    completion: completion.clone(),
                    _batch: batch.clone(),
                    _completed: revoker,
                },
            );
            let running = (|| {
                start(&translated, "Translation.Y", &movement)?;
                start(&object, "Opacity", &fading)?;
                unsafe { method::<NoArgs>(&batch, 2)(batch.as_raw()).ok() }
            })();
            if let Err(error) = running {
                // Remove before stopping: a cancelled batch must never deliver a successful completion.
                states.borrow_mut().remove(&node);
                stop(&translated, "Translation.Y");
                stop(&object, "Opacity");
                _ = unsafe { method::<NoArgs>(&batch, 2)(batch.as_raw()) };
                return Err(error);
            }
            Ok(())
        })();
    if result.is_err() {
        settle(
            element,
            animation.to_y,
            animation.to_opacity.clamp(0.0, 1.0),
        );
    }
    result.map_err(native_error)
}
