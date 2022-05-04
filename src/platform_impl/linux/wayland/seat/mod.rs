//! Seat handling and managing.

use std::cell::RefCell;
use std::rc::Rc;

use sctk::reexports::protocols::unstable::relative_pointer::v1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1;
use sctk::reexports::protocols::unstable::pointer_constraints::v1::client::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1;
use sctk::reexports::protocols::unstable::text_input::v3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;

use sctk::reexports::client::protocol::wl_seat::WlSeat;
use sctk::reexports::client::Attached;

use sctk::environment::Environment;
use sctk::reexports::calloop::LoopHandle;
use sctk::seat::pointer::ThemeManager;
use sctk::seat::{SeatData, SeatListener};
use sctk::data_device::{DataDevice, DndEvent};
use wayland_client::DispatchData;
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;

use super::env::WinitEnv;
use super::event_loop::WinitState;
use crate::event::ModifiersState;

mod keyboard;
pub mod pointer;
pub mod text_input;
mod touch;
mod drop;

use keyboard::Keyboard;
use pointer::Pointers;
use text_input::TextInput;
use touch::Touch;

pub fn drag_and_drop_cb(dnd_event: DndEvent<'_>, dispatch_data: DispatchData<'_>) {
    match dnd_event {
        DndEvent::Enter{..} => println!("Enter..."),
        DndEvent::Motion{..} => println!("Motion..."),
        DndEvent::Drop{..} => println!("Drop..."),
        DndEvent::Leave => println!("Leave..."),
    };
    //println!("{:?}", dnd_event);
    //println!("{:?}", dispatch_data);
}

pub struct SeatManager {
    /// Listener for seats.
    _seat_listener: SeatListener,
}

impl SeatManager {
    pub fn new(
        env: &Environment<WinitEnv>,
        loop_handle: LoopHandle<'static, WinitState>,
        theme_manager: ThemeManager,
    ) -> Self {
        let relative_pointer_manager = env.get_global::<ZwpRelativePointerManagerV1>();
        let pointer_constraints = env.get_global::<ZwpPointerConstraintsV1>();
        let text_input_manager = env.get_global::<ZwpTextInputManagerV3>();
        let data_device_manager = env.get_global::<WlDataDeviceManager>();

        let mut inner = SeatManagerInner::new(
            theme_manager,
            relative_pointer_manager,
            pointer_constraints,
            text_input_manager,
            data_device_manager,
            loop_handle,
        );

        // Handle existing seats.
        for seat in env.get_all_seats() {
            let seat_data = match sctk::seat::clone_seat_data(&seat) {
                Some(seat_data) => seat_data,
                None => continue,
            };

            inner.process_seat_update(&seat, &seat_data);
        }

        let seat_listener = env.listen_for_seats(move |seat, seat_data, _| {
            inner.process_seat_update(&seat, seat_data);
        });

        Self {
            _seat_listener: seat_listener,
        }
    }
}

/// Inner state of the seat manager.
struct SeatManagerInner {
    /// Currently observed seats.
    seats: Vec<SeatInfo>,

    /// Loop handle.
    loop_handle: LoopHandle<'static, WinitState>,

    /// Relative pointer manager.
    relative_pointer_manager: Option<Attached<ZwpRelativePointerManagerV1>>,

    /// Pointer constraints.
    pointer_constraints: Option<Attached<ZwpPointerConstraintsV1>>,

    /// Text input manager.
    text_input_manager: Option<Attached<ZwpTextInputManagerV3>>,

    /// Data device manager.
    data_device_manager: Option<Attached<WlDataDeviceManager>>,

    /// A theme manager.
    theme_manager: ThemeManager,
}

impl SeatManagerInner {
    fn new(
        theme_manager: ThemeManager,
        relative_pointer_manager: Option<Attached<ZwpRelativePointerManagerV1>>,
        pointer_constraints: Option<Attached<ZwpPointerConstraintsV1>>,
        text_input_manager: Option<Attached<ZwpTextInputManagerV3>>,
        data_device_manager: Option<Attached<WlDataDeviceManager>>,
        loop_handle: LoopHandle<'static, WinitState>,
    ) -> Self {
        Self {
            seats: Vec::new(),
            loop_handle,
            relative_pointer_manager,
            pointer_constraints,
            text_input_manager,
            data_device_manager,
            theme_manager,
        }
    }

    /// Handle seats update from the `SeatListener`.
    pub fn process_seat_update(&mut self, seat: &Attached<WlSeat>, seat_data: &SeatData) {
        let detached_seat = seat.detach();

        let position = self.seats.iter().position(|si| si.seat == detached_seat);
        let index = position.unwrap_or_else(|| {
            self.seats.push(SeatInfo::new(detached_seat));
            self.seats.len() - 1
        });

        let seat_info = &mut self.seats[index];

        // Pointer handling.
        if seat_data.has_pointer && !seat_data.defunct {
            if seat_info.pointer.is_none() {
                seat_info.pointer = Some(Pointers::new(
                    seat,
                    &self.theme_manager,
                    &self.relative_pointer_manager,
                    &self.pointer_constraints,
                    seat_info.modifiers_state.clone(),
                ));
            }
        } else {
            seat_info.pointer = None;
        }

        // Handle keyboard.
        if seat_data.has_keyboard && !seat_data.defunct {
            if seat_info.keyboard.is_none() {
                seat_info.keyboard = Keyboard::new(
                    seat,
                    self.loop_handle.clone(),
                    seat_info.modifiers_state.clone(),
                );
            }
        } else {
            seat_info.keyboard = None;
        }

        // Handle touch.
        if seat_data.has_touch && !seat_data.defunct {
            if seat_info.touch.is_none() {
                seat_info.touch = Some(Touch::new(seat));
            }
        } else {
            seat_info.touch = None;
        }

        // Handle text input.
        if let Some(text_input_manager) = self.text_input_manager.as_ref() {
            if seat_data.defunct {
                seat_info.text_input = None;
            } else if seat_info.text_input.is_none() {
                seat_info.text_input = Some(TextInput::new(seat, text_input_manager));
            }
        }

        if let Some(data_device_manager) = self.data_device_manager.as_ref() {
            if seat_info.data_device.is_none() {
                seat_info.data_device = Some(DataDevice::init_for_seat(data_device_manager, seat, drag_and_drop_cb));
            }
        }
    }
}

/// Resources associtated with a given seat.
struct SeatInfo {
    /// Seat to which this `SeatInfo` belongs.
    seat: WlSeat,

    /// A keyboard handle with its repeat rate handling.
    keyboard: Option<Keyboard>,

    /// All pointers we're using on a seat.
    pointer: Option<Pointers>,

    /// Touch handling.
    touch: Option<Touch>,

    /// Text input handling aka IME.
    text_input: Option<TextInput>,

    /// Data device.
    data_device: Option<DataDevice>,

    /// The current state of modifiers observed in keyboard handler.
    ///
    /// We keep modifiers state on a seat, since it's being used by pointer events as well.
    modifiers_state: Rc<RefCell<ModifiersState>>,
}

impl SeatInfo {
    pub fn new(seat: WlSeat) -> Self {
        Self {
            seat,
            keyboard: None,
            pointer: None,
            touch: None,
            text_input: None,
            data_device: None,
            modifiers_state: Rc::new(RefCell::new(ModifiersState::default())),
        }
    }
}
