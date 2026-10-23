//! The transcript's measure (design plan §5): a widget holding one child at the left of its room,
//! at most [`style::TRANSCRIPT_MEASURE_CHARS`] characters of the message text wide, so the room
//! past it on a wide window stays empty on the right.

use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::style;

/// The message text's size (`.nuncio-text` in [`crate::style`]): the measure counts its characters.
const TEXT_PX: f64 = 13.0;

glib::wrapper! {
    /// Holds its one child at the left, at most the transcript's measure wide.
    pub struct Measure(ObjectSubclass<imp::Measure>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Measure {
    /// A measure around `child`.
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let measure: Self = glib::Object::new();
        child.set_parent(&measure);
        measure
    }

    /// The widest the child may be in `available` pixels: the measure, in the font in use now,
    /// so a font switch moves it.
    fn width_in(&self, child: &gtk::Widget, available: i32) -> i32 {
        let context = self.pango_context();
        let mut font = context.font_description().unwrap_or_default();
        font.set_absolute_size(TEXT_PX * f64::from(pango::SCALE));
        let metrics = context.metrics(Some(&font), None);
        let char_width = metrics
            .approximate_char_width()
            .max(metrics.approximate_digit_width())
            / pango::SCALE;
        let (min, _, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
        style::transcript_width(available, char_width, min)
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Measure;

    #[glib::object_subclass]
    impl ObjectSubclass for Measure {
        const NAME: &'static str = "NuncioMeasure";
        type Type = super::Measure;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Measure {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Measure {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let obj = self.obj();
            let Some(child) = obj.first_child() else {
                return (0, 0, -1, -1);
            };
            // Its height is the child's at the width the child gets.
            let for_size = if orientation == gtk::Orientation::Vertical && for_size >= 0 {
                obj.width_in(&child, for_size)
            } else {
                for_size
            };
            let (min, natural, _, _) = child.measure(orientation, for_size);
            (min, natural, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let obj = self.obj();
            if let Some(child) = obj.first_child() {
                let width = obj.width_in(&child, width);
                child.size_allocate(&gtk::Allocation::new(0, 0, width, height), -1);
            }
        }
    }
}
