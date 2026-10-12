use gtk::{glib, prelude::*, subclass::prelude::*};

mod imp {
    use gtk::{Orientation, SizeRequestMode, Widget, glib, prelude::*, subclass::prelude::*};
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct Clamp {
        pub child: RefCell<Option<Widget>>,
        pub maximum_width: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Clamp {
        const NAME: &'static str = "SplayClamp";
        type Type = super::Clamp;
        type ParentType = Widget;
    }

    impl ObjectImpl for Clamp {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Clamp {
        fn request_mode(&self) -> SizeRequestMode {
            SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.child.borrow().clone() else {
                return (0, 0, -1, -1);
            };

            if orientation == Orientation::Horizontal {
                let (minimum, natural, _, _) = child.measure(orientation, for_size);
                return (
                    minimum,
                    natural.min(self.child_width(&child, i32::MAX)),
                    -1,
                    -1,
                );
            }

            let for_width = if for_size < 0 {
                -1
            } else {
                self.child_width(&child, for_size)
            };
            let (minimum, natural, _, _) = child.measure(orientation, for_width);
            (minimum, natural, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.child.borrow().clone() else {
                return;
            };

            let child_width = self.child_width(&child, width);
            let x = ((width - child_width) / 2).max(0);
            child.size_allocate(&gtk::Allocation::new(x, 0, child_width, height), baseline);
        }
    }

    impl Clamp {
        /// Width the child gets out of `available`: capped at the maximum,
        /// but never below what the child needs.
        fn child_width(&self, child: &Widget, available: i32) -> i32 {
            let (minimum, _, _, _) = child.measure(Orientation::Horizontal, -1);
            available.min(self.maximum_width.get()).max(minimum)
        }
    }
}

glib::wrapper! {
    /// A container that gives its child at most a maximum width and centers
    /// it, like libadwaita's `AdwClamp`.
    pub struct Clamp(ObjectSubclass<imp::Clamp>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Clamp {
    pub fn new(maximum_width: i32, child: &impl IsA<gtk::Widget>) -> Self {
        let clamp: Self = glib::Object::new();
        clamp.imp().maximum_width.set(maximum_width);
        child.set_parent(&clamp);
        *clamp.imp().child.borrow_mut() = Some(child.clone().upcast());
        clamp
    }
}
