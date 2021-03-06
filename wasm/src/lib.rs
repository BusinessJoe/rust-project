use image::RgbaImage;

use js_sys::ArrayBuffer;
use wasm_bindgen::prelude::*;
use wasm_bindgen::Clamped;
use web_sys::{console, ImageData};

use shape_evolution::evolve::epoch;
use shape_evolution::random_shape::{RandomCircle, RandomShape};

mod utils;
pub mod web;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct TestStruct {
    target_img: image::RgbaImage,
    current_img: image::RgbaImage,
    current_score: u128,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl TestStruct {
    fn new_from_image(target_img: RgbaImage) -> Self {
        let (width, height) = target_img.dimensions();

        // Scale the target image down to an appropriate size
        // 300 × 300 = 90,000 pixels seems good enough
        const TARGET_NUM_PIXELS: u32 = 200 * 200;
        let target_scale_factor: f64 =
            (f64::from(width * height) / f64::from(TARGET_NUM_PIXELS)).sqrt();
        let target_img = image::imageops::resize(
            &target_img,
            (f64::from(width) / target_scale_factor) as u32,
            (f64::from(height) / target_scale_factor) as u32,
            image::imageops::FilterType::Nearest,
        );
        let (width, height) = target_img.dimensions();

        Self {
            target_img,
            current_img: RgbaImage::new(width, height),
            current_score: u128::from(width * height * 255 * 3),
        }
    }

    pub async fn new_async(url: String) -> Self {
        utils::set_panic_hook();

        let target_img = web::load_image(&url).await.unwrap();
        Self::new_from_image(target_img)
    }

    pub fn new_from_buffer(buffer: ArrayBuffer) -> Result<TestStruct, JsValue> {
        utils::set_panic_hook();

        let target_img = web::load_image_from_buffer(&buffer)?;
        Ok(TestStruct::new_from_image(target_img))
    }

    pub fn get_image_data(&self) -> Result<JsValue, JsValue> {
        let (width, height) = self.current_img.dimensions();
        let data = self.current_img.to_vec();

        let data = ImageData::new_with_u8_clamped_array_and_sh(Clamped(&data), width, height)?;
        Ok(JsValue::from(data))
    }

    pub fn try_epoch(&mut self, generation_size: usize, num_gens: u32) -> Option<RandomCircle> {
        match epoch(
            generation_size,
            num_gens,
            &self.target_img,
            &self.current_img,
            self.current_score,
        ) {
            Some((best_shape, new_score)) => {
                self.current_score = new_score;
                self.current_img = best_shape.draw(&self.current_img);

                Some(best_shape)
            }
            None => {
                console::log_1(&JsValue::from_str("Discarded circle"));
                None
            }
        }
    }

    pub fn get_target_width(&self) -> u32 {
        self.target_img.width()
    }

    pub fn get_target_height(&self) -> u32 {
        self.target_img.height()
    }
}
