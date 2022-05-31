use crate::image_diff::image_diff;
use image::GenericImageView;
use image::Pixel;
use rand::Rng;
use std::cmp;

#[derive(Debug, PartialEq)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub trait RandomShape {
    #[must_use]
    fn draw(&self, image: &image::RgbaImage, scale: f64) -> image::RgbaImage;

    // Returns the same output as draw, but cropped to the bounding box returned by
    // get_bounds().
    #[must_use]
    fn draw_subimage(&self, image: &image::RgbaImage, scale: f64) -> image::RgbaImage;

    #[must_use]
    fn mutate(&self) -> Self;

    #[must_use]
    fn get_bounds(&self, scale: f64) -> Option<BoundingBox>;

    // Calculates and returns how close the current image becomes to the target after this shape is
    // drawn. Smaller scores are better.
    #[must_use]
    fn score(
        &self,
        target_img: &image::RgbaImage,
        current_img: &image::RgbaImage,
        prev_score: i64,
        scale: f64,
    ) -> i64;
}

#[derive(Clone, Debug)]
pub struct RandomCircle {
    pub imgx: u32,
    pub imgy: u32,
    pub center: (i32, i32),
    pub radius: i32,
    pub color: image::Rgba<u8>,
}

#[must_use]
fn clamp_channel(c: i32) -> u8 {
    cmp::max(0, cmp::min(255, c)) as u8
}

#[must_use]
fn mutate_center(center: (i32, i32), rng: &mut rand::rngs::ThreadRng) -> (i32, i32) {
    let dc1 = rng.gen_range(-5..=5);
    let dc2 = rng.gen_range(-5..=5);
    (center.0 + dc1, center.1 + dc2)
}

#[must_use]
fn mutate_radius(radius: i32, rng: &mut rand::rngs::ThreadRng) -> i32 {
    let drad = rng.gen_range(-20..=2);
    cmp::max(radius + drad, 1)
}

#[must_use]
fn mutate_color(color: image::Rgba<u8>, rng: &mut rand::rngs::ThreadRng) -> image::Rgba<u8> {
    let dr = rng.gen_range(-20..=20);
    let dg = rng.gen_range(-20..=20);
    let db = rng.gen_range(-20..=20);

    let r = clamp_channel(i32::from(color.channels()[0]) + dr);
    let g = clamp_channel(i32::from(color.channels()[1]) + dg);
    let b = clamp_channel(i32::from(color.channels()[2]) + db);

    image::Rgba([r, g, b, 255])
}

impl RandomShape for RandomCircle {
    fn draw(&self, image: &image::RgbaImage, scale: f64) -> image::RgbaImage {
        let center = (
            (self.center.0 as f64 * scale) as i32,
            (self.center.1 as f64 * scale) as i32,
        );
        let radius = (self.radius as f64 * scale) as i32;
        imageproc::drawing::draw_filled_circle(image, center, radius, self.color)
    }

    fn draw_subimage(&self, image: &image::RgbaImage, scale: f64) -> image::RgbaImage {
        let bounds = self.get_bounds(scale).unwrap();
        let image = image
            .view(bounds.x, bounds.y, bounds.width, bounds.height)
            .to_image();
        let center = (
            (self.center.0 as f64 * scale - bounds.x as f64) as i32,
            (self.center.1 as f64 * scale - bounds.y as f64) as i32,
        );
        let radius = (self.radius as f64 * scale) as i32;
        // Pass a reference to image, since the new value of image is no longer a reference.
        imageproc::drawing::draw_filled_circle(&image, center, radius, self.color)
    }

    fn mutate(&self) -> Self {
        let mut rng = rand::thread_rng();

        let center = mutate_center(self.center, &mut rng);
        let color = mutate_color(self.color, &mut rng);
        let radius = mutate_radius(self.radius, &mut rng);
        Self {
            imgx: self.imgx,
            imgy: self.imgy,
            center,
            radius,
            color,
        }
    }

    fn get_bounds(&self, scale: f64) -> Option<BoundingBox> {
        let x = cmp::max(self.center.0 - self.radius - 1, 0);
        let y = cmp::max(self.center.1 - self.radius - 1, 0);
        let x2 = cmp::min(self.center.0 + self.radius + 1, (self.imgx - 1) as i32);
        let y2 = cmp::min(self.center.1 + self.radius + 1, (self.imgy - 1) as i32);

        // Return none if bounds are not contained within image.
        if x >= self.imgx.try_into().unwrap()
            || y >= self.imgy.try_into().unwrap()
            || x2 < 0
            || y2 < 0
        {
            return None;
        }

        Some(BoundingBox {
            x: (x as f64 * scale) as u32,
            y: (y as f64 * scale) as u32,
            width: ((x2 - x + 1) as f64 * scale) as u32,
            height: ((y2 - y + 1) as f64 * scale) as u32,
        })
    }

    fn score(
        &self,
        target_img: &image::RgbaImage,
        current_img: &image::RgbaImage,
        prev_score: i64,
        scale: f64,
    ) -> i64 {
        let (imgx, imgy) = target_img.dimensions();
        let bounds = match self.get_bounds(scale) {
            Some(b) => b,
            None => return prev_score, // If the bounds lay outside the image, this shape does not change the image
        };

        // Compare the area of the bounding box to the area of the target image - if the bounding
        // box is sufficiently small, use the scoring algorithm for smaller shapes.
        if bounds.width * bounds.height < imgx * imgy / 4 {
            self.score_small(target_img, current_img, scale, prev_score)
        } else {
            self.score_large(target_img, current_img, scale)
        }
    }
}

impl RandomCircle {
    #[must_use]
    pub fn new(imgx: u32, imgy: u32) -> Self {
        let simgx = imgx as i32;
        let simgy = imgy as i32;

        let mut rng = rand::thread_rng();
        let max_radius = cmp::max(simgx, simgy);

        Self {
            imgx,
            imgy,
            center: (rng.gen_range(0..simgx), rng.gen_range(0..simgy)),
            radius: rng.gen_range(1..max_radius),
            color: image::Rgba([
                rng.gen_range(0..=255),
                rng.gen_range(0..=255),
                rng.gen_range(0..=255),
                255,
            ]),
        }
    }

    // We can use the bounds of the shape to crop the target and current image to a smaller area
    // where all the drawing and scoring can be done. This greatly improves performance on shapes
    // with smaller bounding boxes.
    fn score_small(
        &self,
        target_img: &image::RgbaImage,
        current_img: &image::RgbaImage,
        scale: f64,
        prev_score: i64,
    ) -> i64 {
        let bounds = self.get_bounds(scale).unwrap();

        let cropped_target = target_img
            .view(bounds.x, bounds.y, bounds.width, bounds.height)
            .to_image();
        let cropped_current = current_img
            .view(bounds.x, bounds.y, bounds.width, bounds.height)
            .to_image();

        let new_img = self.draw_subimage(current_img, scale);

        let prev_cropped_score = image_diff(&cropped_target, &cropped_current);
        let new_cropped_score = image_diff(&cropped_target, &new_img);

        prev_score + new_cropped_score - prev_cropped_score
    }

    // On shapes with large bounding boxes, it's best to avoid cropping and simply draw and score
    // on the original target image.
    fn score_large(
        &self,
        target_img: &image::RgbaImage,
        current_img: &image::RgbaImage,
        scale: f64,
    ) -> i64 {
        let new_img = self.draw(current_img, scale);
        image_diff(target_img, &new_img)
    }
}

#[cfg(test)]
mod tests {
    use crate::random_shape::{image_diff, BoundingBox, RandomShape};
    use crate::RandomCircle;
    use image::RgbaImage;
    use std::iter;

    fn assert_scoring_equal(
        shape: &RandomCircle,
        target_img: &image::RgbaImage,
        current_img: &image::RgbaImage,
        prev_score: i64,
        scale: f64,
    ) {
        match shape.get_bounds(scale) {
            Some(_b) => {}
            None => return,
        };
        let score_small = shape.score_small(target_img, current_img, scale, prev_score);
        let score_large = shape.score_large(target_img, current_img, scale);
        assert_eq!(score_small, score_large);
    }

    #[test]
    fn test_scoring_algs_equal() {
        let (imgx, imgy) = (50, 75);

        // Create 1000 random shapes for testing
        let shapes = iter::repeat_with(|| RandomCircle::new(imgx, imgy)).take(1000);

        let target_img = RgbaImage::new(imgx, imgy);
        let current_img = RgbaImage::new(imgx, imgy);
        let prev_score = image_diff(&target_img, &current_img);

        for shape in shapes {
            assert_scoring_equal(&shape, &target_img, &current_img, prev_score, 1.0);
        }
    }

    #[test]
    fn test_scoring_algs_equal_scale_5() {
        let (imgx, imgy) = (10, 15);
        let scale = 5;

        // Create 1000 random shapes for testing
        let shapes = iter::repeat_with(|| RandomCircle::new(imgx, imgy)).take(1000);

        let target_img = RgbaImage::new(imgx * scale, imgy * scale);
        let current_img = RgbaImage::new(imgx * scale, imgy * scale);
        let prev_score = image_diff(&target_img, &current_img);

        assert_eq!(prev_score, 0);

        for shape in shapes {
            assert_scoring_equal(&shape, &target_img, &current_img, prev_score, scale as f64);
        }
    }

    #[test]
    fn test_score_algs_equal_shape_outside_canvas() {
        let (imgx, imgy) = (50, 75);

        let target_img = RgbaImage::new(imgx, imgy);
        let current_img = RgbaImage::new(imgx, imgy);
        let prev_score = image_diff(&target_img, &current_img);

        let shape = RandomCircle {
            imgx,
            imgy,
            center: (-100, -100),
            radius: 1,
            color: image::Rgba([255, 255, 255, 255]),
        };
        assert_scoring_equal(&shape, &target_img, &current_img, prev_score, 1.0);
    }

    #[test]
    fn test_shape_fills_canvas_bounds() {
        let (imgx, imgy) = (50, 75);

        let shape = RandomCircle {
            imgx,
            imgy,
            center: (100, 100),
            radius: 1000,
            color: image::Rgba([255, 255, 255, 255]),
        };

        let expected_bounds = BoundingBox {
            x: 0,
            y: 0,
            width: imgx,
            height: imgy,
        };

        assert_eq!(shape.get_bounds(1.0), Some(expected_bounds));
    }

    #[test]
    fn test_score_small_shape_fills_canvas() {
        let (imgx, imgy) = (50, 75);

        let target_img = RgbaImage::new(imgx, imgy);
        let current_img = RgbaImage::new(imgx, imgy);
        let prev_score = image_diff(&target_img, &current_img);

        let shape = RandomCircle {
            imgx,
            imgy,
            center: (100, 100),
            radius: 1000,
            color: image::Rgba([255, 255, 255, 255]),
        };
        assert_eq!(
            shape.score_small(&target_img, &current_img, 1.0, prev_score),
            (imgx * imgy * 255 * 3) as i64
        );
    }

    #[test]
    fn test_score_large_shape_fills_canvas() {
        let (imgx, imgy) = (50, 75);

        let target_img = RgbaImage::new(imgx, imgy);
        let current_img = RgbaImage::new(imgx, imgy);
        let prev_score = image_diff(&target_img, &current_img);

        assert_eq!(prev_score, 0);

        let shape = RandomCircle {
            imgx,
            imgy,
            center: (100, 100),
            radius: 1000,
            color: image::Rgba([255, 255, 255, 255]),
        };
        assert_eq!(
            shape.score_large(&target_img, &current_img, 1.0),
            (imgx * imgy * 255 * 3) as i64
        );
    }

    #[test]
    fn test_score_algs_equal_shape_fills_canvas() {
        let (imgx, imgy) = (50, 75);

        let target_img = RgbaImage::new(imgx, imgy);
        let current_img = RgbaImage::new(imgx, imgy);
        let prev_score = image_diff(&target_img, &current_img);

        let shape = RandomCircle {
            imgx,
            imgy,
            center: (100, 100),
            radius: 1000,
            color: image::Rgba([255, 255, 255, 255]),
        };
        assert_scoring_equal(&shape, &target_img, &current_img, prev_score, 1.0);
    }
}
