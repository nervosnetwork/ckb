use tui::buffer::Buffer;
use tui::layout::{Corner, Rect};
use tui::style::Style;
use tui::widgets::{Block, Text, Widget};

pub struct List<'b, L>
where
    L: Iterator<Item = Text<'b>>,
{
    block: Option<Block<'b>>,
    items: L,
    style: Style,
    start_corner: Corner,
}

impl<'b, L> Default for List<'b, L>
where
    L: Iterator<Item = Text<'b>> + Default,
{
    fn default() -> List<'b, L> {
        List {
            block: None,
            items: L::default(),
            style: Default::default(),
            start_corner: Corner::TopLeft,
        }
    }
}

impl<'b, L> List<'b, L>
where
    L: Iterator<Item = Text<'b>>,
{
    pub fn new(items: L) -> List<'b, L> {
        List {
            block: None,
            items,
            style: Default::default(),
            start_corner: Corner::TopLeft,
        }
    }

    // pub fn block(mut self, block: Block<'b>) -> List<'b, L> {
    //     self.block = Some(block);
    //     self
    // }
}

impl<'b, L> Widget for List<'b, L>
where
    L: Iterator<Item = Text<'b>>,
{
    fn draw(&mut self, area: Rect, buf: &mut Buffer) {
        let list_area = match self.block {
            Some(ref mut b) => {
                b.draw(area, buf);
                b.inner(area)
            }
            None => area,
        };

        if list_area.width < 1 || list_area.height < 1 {
            return;
        }

        self.background(list_area, buf, self.style.bg);

        for (i, item) in self
            .items
            .by_ref()
            .enumerate()
            .take(list_area.height as usize)
        {
            let (x, y) = match self.start_corner {
                Corner::TopLeft => (list_area.left(), list_area.top() + i as u16),
                Corner::BottomLeft => (list_area.left(), list_area.bottom() - (i + 1) as u16),
                // Not supported
                _ => (list_area.left(), list_area.top() + i as u16),
            };
            match item {
                Text::Raw(ref v) => {
                    buf.set_stringn(x, y, v, list_area.width as usize, Style::default());
                }
                Text::Styled(ref v, s) => {
                    buf.set_stringn(x, y, v, list_area.width as usize, s);
                }
            };
        }
    }
}
