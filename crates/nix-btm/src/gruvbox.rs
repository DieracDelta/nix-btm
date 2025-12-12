//MIT License
//Copyright (c) 2024 Adrian Papari
//
//Permission is hereby granted, free of charge, to any person obtaining a copy
//of this software and associated documentation files (the "Software"), to deal
//in the Software without restriction, including without limitation the rights
//to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//copies of the Software, and to permit persons to whom the Software is
//furnished to do so, subject to the following conditions:
//
//The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
//THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//SOFTWARE.
//
// copied at verbatim from https://github.com/junkdog/tachyonfx
use ratatui::prelude::*;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(dead_code)]
pub enum Gruvbox {
    Dark0Hard,
    Dark0,
    Dark0Soft,
    Dark1,
    Dark2,
    Dark3,
    Dark4,
    Gray245,
    Gray244,
    Light0Hard,
    Light0,
    Light0Soft,
    Light1,
    Light2,
    Light3,
    Light4,
    RedBright,
    GreenBright,
    YellowBright,
    BlueBright,
    PurpleBright,
    AquaBright,
    OrangeBright,
    Red,
    Green,
    Yellow,
    Blue,
    Purple,
    Aqua,
    Orange,
    RedDim,
    GreenDim,
    YellowDim,
    BlueDim,
    PurpleDim,
    AquaDim,
    OrangeDim,
}

impl Gruvbox {
    const fn color(&self) -> Color {
        match self {
            Self::Dark0Hard => Color::from_u32(0x1d2021),
            Self::Dark0 => Color::from_u32(0x282828),
            Self::Dark0Soft => Color::from_u32(0x32302f),
            Self::Dark1 => Color::from_u32(0x3c3836),
            Self::Dark2 => Color::from_u32(0x504945),
            Self::Dark3 => Color::from_u32(0x665c54),
            Self::Dark4 => Color::from_u32(0x7c6f64),
            Self::Gray245 => Color::from_u32(0x928374),
            Self::Gray244 => Color::from_u32(0x928374),
            Self::Light0Hard => Color::from_u32(0xf9f5d7),
            Self::Light0 => Color::from_u32(0xfbf1c7),
            Self::Light0Soft => Color::from_u32(0xf2e5bc),
            Self::Light1 => Color::from_u32(0xebdbb2),
            Self::Light2 => Color::from_u32(0xd5c4a1),
            Self::Light3 => Color::from_u32(0xbdae93),
            Self::Light4 => Color::from_u32(0xa89984),
            Self::RedBright => Color::from_u32(0xfb4934),
            Self::GreenBright => Color::from_u32(0xb8bb26),
            Self::YellowBright => Color::from_u32(0xfabd2f),
            Self::BlueBright => Color::from_u32(0x83a598),
            Self::PurpleBright => Color::from_u32(0xd3869b),
            Self::AquaBright => Color::from_u32(0x8ec07c),
            Self::OrangeBright => Color::from_u32(0xfe8019),
            Self::Red => Color::from_u32(0xcc241d),
            Self::Green => Color::from_u32(0x98971a),
            Self::Yellow => Color::from_u32(0xd79921),
            Self::Blue => Color::from_u32(0x458588),
            Self::Purple => Color::from_u32(0xb16286),
            Self::Aqua => Color::from_u32(0x689d6a),
            Self::Orange => Color::from_u32(0xd65d0e),
            Self::RedDim => Color::from_u32(0x9d0006),
            Self::GreenDim => Color::from_u32(0x79740e),
            Self::YellowDim => Color::from_u32(0xb57614),
            Self::BlueDim => Color::from_u32(0x076678),
            Self::PurpleDim => Color::from_u32(0x8f3f71),
            Self::AquaDim => Color::from_u32(0x427b58),
            Self::OrangeDim => Color::from_u32(0xaf3a03),
        }
    }
}

impl From<Gruvbox> for Color {
    fn from(val: Gruvbox) -> Self {
        val.color()
    }
}
