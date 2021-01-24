use lindera::tokenizer::Tokenizer;
use lindera_core::core::viterbi::Mode;
use regex::Regex;
use std::io::Write;
use cairo::{ImageSurface, Context, Format, FontFace, FontSlant, FontWeight};
use std::io;

const W: usize = 40;
const FONT_SIZE: usize = 30;

enum Error {
    CairoError,
    IoError,
}

fn calc_split(text_ja: &str) -> Vec<String> {
    let mut tokenizer = Tokenizer::new(Mode::Normal, "");
    let tokens = tokenizer.tokenize(text_ja);
    let mut costs = Vec::new();
    costs.resize(tokens.len() + 1, 0);

    let sep_re = Regex::new(r"[、,\.。．…‥，・]").unwrap();
    let space_re = Regex::new(r"[ 　\n\r\t]").unwrap();
    let open_re = Regex::new(r"[\(\[\{【「（]").unwrap();
    let close_re = Regex::new(r"[\)\]\}】」）]").unwrap();

    costs.resize(tokens.len() + 1, 0);
    println!("{}", sep_re.is_match("."));
    tokens.iter().enumerate().for_each(|(i, token)| {
        if sep_re.is_match(token.text) {
            costs[i] = -6;
            costs[i + 1] = 6;
        } else if space_re.is_match(token.text) {
            costs[i] = 6;
            costs[i + 1] = 6;
        } else if open_re.is_match(token.text) {
            costs[i] = 6;
            costs[i + 1] = -6;
        } else if close_re.is_match(token.text) {
            costs[i] = -6;
            costs[i + 1] = 6;
        }
    });
    let mut lines = Vec::new();
    let mut line = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if (line.len() + token.text.len()) as i64 + costs[i] > W as i64 {
            lines.push(line);
            line = token.text.to_owned();
        } else {
            line.push_str(token.text);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn render<W: Write>(w: &mut W, title: &str) -> Result<(), io::Error> {
    let w = FONT_SIZE * (W + 6);
    let h = FONT_SIZE * (W + 6) / 3 * 2;
    let surface = ImageSurface::create(Format::Rgb24, w, h).unwrap();
    let context = Context::new(&surface);
    context.set_source_rgb(1.0, 1.0, 1.0);
    context.rectangle(0.0, 0.0, w, h);
    context.fill();
    context.set_font_face(FontFace::toy_create("Noto Sans CJK JP", FontSlant::Normal, FontWeight::Normal));
    let lines = calc_split(title);
    context.move_to(0.0, h - FONT_SIZE as f64 * 2);
    for line in lines.iter().rev() {
        context.show_text(line);
        context.rel_move_to(0.0, -FONT_SIZE as f64);
    }
    surface.write_to_png(w);
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_calc_split() {
        let src = concat!(
            "日本国民は、正当に選挙された国会における代表者を通じて行動し、",
            "われらとわれらの子孫のために、諸国民との協和による成果と、",
            "わが国全土にわたつて自由のもたらす恵沢を確保し、",
            "政府の行為によつて再び戦争の惨禍が起ることのないやうにすることを決意し、",
            "ここに主権が国民に存することを宣言し、この憲法を確定する。"
        );
        assert_eq!(
            calc_split(&src),
            vec![
                "日本国民は、正当に選挙され",
                "た国会における代表者",
                "を通じて行動し、われらと",
                "われらの子孫のために、",
                "諸国民との協和による成果と、",
                "わが国全土にわたつて自由の",
                "もたらす恵沢を確保し、",
                "政府の行為によつて再び戦争",
                "の惨禍が起ることのないやう",
                "にすることを決意し、",
                "ここに主権が国民に存する",
                "ことを宣言し、この憲法を",
                "確定する。"
            ]
        );
    }

    use std::fs;
    #[test]
    fn test_draw() {
        let f = fs::File::create("out.png");
        render(&mut f, "日本国民は、正当に選挙された国会における代表者を通じて行動し、").unwrap();
    }
}
