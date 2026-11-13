
#[derive(Debug)]
struct Node {
    x: usize,
    y: usize,
    width: usize,
}

#[derive(Debug)]
pub struct Atlas {
    width: usize,
    height: usize,
    nodes: Vec<Node>,
}

impl Atlas {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            nodes: vec![Node { x: 0, y: 0, width }],
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn add_rect(&mut self, rect_width: usize, rect_height: usize) -> Option<(usize, usize)> {
        let mut besth = self.height;
        let mut bestw = self.width;
        let mut besti = None;
        let mut bestx = 0;
        let mut besty = 0;

        for i in 0..self.nodes.len() {
            if let Some(y) = self.rect_fits(i, rect_width, rect_height) {
                if y + rect_height < besth || (y + rect_height == besth && self.nodes[i].width < bestw) {
                    besti = Some(i);
                    bestw = self.nodes[i].width;
                    besth = y + rect_height;
                    bestx = self.nodes[i].x;
                    besty = y;
                }
            }
        }

        if let Some(besti) = besti {
            self.add_skyline_level(besti, bestx, besty, rect_width, rect_height);
            return Some((bestx, besty));
        }

        None
    }

    fn insert_node(&mut self, idx: usize, x: usize, y: usize, width: usize) {
        self.nodes.insert(idx, Node { x, y, width });
    }

    fn remove_node(&mut self, idx: usize) {
        self.nodes.remove(idx);
    }

    fn add_skyline_level(&mut self, idx: usize, x: usize, y: usize, width: usize, height: usize) {
        self.insert_node(idx, x, y + height, width);

        let mut i = idx + 1;

        while i < self.nodes.len() {
            if self.nodes[i].x < self.nodes[i - 1].x + self.nodes[i - 1].width {
                let shrink = self.nodes[i - 1].x + self.nodes[i - 1].width - self.nodes[i].x;

                self.nodes[i].x += shrink;
                let new_width = self.nodes[i].width as isize - shrink as isize;

                if new_width <= 0 {
                    self.remove_node(i);
                    i -= 1;
                } else {
                    self.nodes[i].width = new_width as usize;
                    break;
                }
            } else {
                break;
            }

            i += 1;
        }

        let mut i = 0isize;

        while i < self.nodes.len() as isize - 1 {
            let index = i as usize;

            if self.nodes[index].y == self.nodes[index + 1].y {
                self.nodes[index].width += self.nodes[index + 1].width;
                self.remove_node(index + 1);

                i -= 1;
            }

            i += 1;
        }
    }

    fn rect_fits(&self, mut idx: usize, width: usize, height: usize) -> Option<usize> {
        let x = self.nodes[idx].x;
        let mut y = self.nodes[idx].y;

        if x + width > self.width {
            return None;
        }

        let mut space_left = width as isize;

        while space_left > 0 {
            if idx == self.nodes.len() {
                return None;
            }

            y = y.max(self.nodes[idx].y);

            if y + height > self.height {
                return None;
            }

            space_left -= self.nodes[idx].width as isize;
            idx += 1;
        }

        Some(y)
    }
}
