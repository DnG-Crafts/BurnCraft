//! EPS and PostScript import: a compact PostScript interpreter that runs the file's program and
//! keeps what it fills and strokes as outlines (mm), coloured as drawn. It covers the language
//! (stacks, dictionaries, procedures, loops, `stopped`, strings and arrays), the coordinate system
//! and the path operators, and the short operators of Illustrator's legacy format (`m l c v y f s
//! b …`, used when a file names a prolog it doesn't include). Text and pictures aren't drawn (each
//! is reported once); clipping is ignored. A program that stops on an error keeps what it drew.
//!
//! The page is the file's `%%HiResBoundingBox` / `%%BoundingBox` (else the art's own bounds); y
//! is flipped so the page's top-left corner is the origin.

use crate::{ImportResult, Imported};
use bc_geom::{Affine, BezPath, PathEl, Point, Rect, Vec2};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// File extensions read as PostScript.
pub const EXTENSIONS: [&str; 2] = ["eps", "ps"];

const MM_PER_PT: f64 = 25.4 / 72.0;
/// Most operations one file may run, and how deep procedures may nest.
const MAX_OPS: u64 = 40_000_000;
const MAX_DEPTH: usize = 400;
const MAX_STACK: usize = 100_000;
const MAX_SHAPES: usize = 1 << 20;

type Key = Rc<str>;
type Dict = Rc<RefCell<HashMap<Key, Obj>>>;
type Arr = Rc<RefCell<Vec<Obj>>>;

#[derive(Clone)]
enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// A name, executable or literal.
    Name(Key, bool),
    Str(Rc<RefCell<Vec<u8>>>),
    /// An array, executable (a procedure) or literal.
    Array(Arr, bool),
    Dict(Dict),
    Op(Key),
    Mark,
    /// The program's own text, or data filtered from it.
    File,
    Save,
}

impl Obj {
    fn num(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(*i as f64),
            Obj::Real(r) => Some(*r),
            _ => None,
        }
    }
    fn name(s: &str) -> Obj {
        Obj::Name(s.into(), false)
    }
    fn string(b: Vec<u8>) -> Obj {
        Obj::Str(Rc::new(RefCell::new(b)))
    }
    fn array(v: Vec<Obj>) -> Obj {
        Obj::Array(Rc::new(RefCell::new(v)), false)
    }
    fn type_name(&self) -> &'static str {
        match self {
            Obj::Null => "nulltype",
            Obj::Bool(_) => "booleantype",
            Obj::Int(_) => "integertype",
            Obj::Real(_) => "realtype",
            Obj::Name(..) => "nametype",
            Obj::Str(_) => "stringtype",
            Obj::Array(..) => "arraytype",
            Obj::Dict(_) => "dicttype",
            Obj::Op(_) => "operatortype",
            Obj::Mark => "marktype",
            Obj::File => "filetype",
            Obj::Save => "savetype",
        }
    }
    /// The text of a name or string (dictionary keys, `cvs`).
    fn text(&self) -> String {
        match self {
            Obj::Name(n, _) | Obj::Op(n) => n.to_string(),
            Obj::Str(s) => String::from_utf8_lossy(&s.borrow()).into_owned(),
            Obj::Int(i) => i.to_string(),
            Obj::Real(r) => format!("{r}"),
            Obj::Bool(b) => b.to_string(),
            Obj::Null => "null".into(),
            o => format!("--{}--", o.type_name()),
        }
    }
    fn key(&self) -> Key {
        self.text().into()
    }
}

/// Why execution stopped.
enum Ps {
    Stop,
    Exit,
    Quit,
    Error(String),
}

type Res<T = ()> = Result<T, Ps>;

fn err<T>(s: impl Into<String>) -> Res<T> {
    Err(Ps::Error(s.into()))
}

// ---------------------------------------------------------------- scanner

struct Lexer {
    src: Rc<[u8]>,
    at: usize,
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%') || b.is_ascii_whitespace() || b == 0
}

impl Lexer {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while let Some(b) = self.peek() {
            if b == b'%' {
                while let Some(c) = self.peek() {
                    self.at += 1;
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                }
            } else if b.is_ascii_whitespace() || b == 0 {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self.peek().is_some_and(|b| !is_delim(b)) {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.src[start..self.at]).into_owned()
    }

    fn string(&mut self) -> Res<Vec<u8>> {
        let mut out = Vec::new();
        let mut depth = 1;
        loop {
            let Some(b) = self.peek() else { return err("syntaxerror: unterminated string") };
            self.at += 1;
            match b {
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                    out.push(b);
                }
                b'\\' => {
                    let Some(c) = self.peek() else { continue };
                    self.at += 1;
                    match c {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.at += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut v = (c - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.at += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        c => out.push(c),
                    }
                }
                b => out.push(b),
            }
        }
    }

    fn hex(&mut self) -> Vec<u8> {
        let mut digits = Vec::new();
        while let Some(b) = self.peek() {
            self.at += 1;
            if b == b'>' {
                break;
            }
            if let Some(d) = (b as char).to_digit(16) {
                digits.push(d as u8);
            }
        }
        if digits.len() % 2 == 1 {
            digits.push(0);
        }
        digits.chunks(2).map(|p| p[0] << 4 | p[1]).collect()
    }

    fn base85(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut group: Vec<u32> = Vec::new();
        while let Some(b) = self.peek() {
            self.at += 1;
            match b {
                b'~' => {
                    if self.peek() == Some(b'>') {
                        self.at += 1;
                    }
                    break;
                }
                b'z' if group.is_empty() => out.extend([0; 4]),
                b'!'..=b'u' => {
                    group.push((b - b'!') as u32);
                    if group.len() == 5 {
                        let v = group.iter().fold(0u32, |a, d| a.wrapping_mul(85).wrapping_add(*d));
                        out.extend(v.to_be_bytes());
                        group.clear();
                    }
                }
                _ => {}
            }
        }
        if !group.is_empty() {
            let n = group.len();
            group.resize(5, 84);
            let v = group.iter().fold(0u32, |a, d| a.wrapping_mul(85).wrapping_add(*d));
            out.extend(&v.to_be_bytes()[..n - 1]);
        }
        out
    }

    /// The next token: `None` at the end.
    fn token(&mut self) -> Res<Option<Obj>> {
        self.skip_space();
        let Some(b) = self.peek() else { return Ok(None) };
        let o = match b {
            b'(' => {
                self.at += 1;
                Obj::string(self.string()?)
            }
            b'<' => {
                self.at += 1;
                match self.peek() {
                    Some(b'<') => {
                        self.at += 1;
                        Obj::Name("<<".into(), true)
                    }
                    Some(b'~') => {
                        self.at += 1;
                        Obj::string(self.base85())
                    }
                    _ => Obj::string(self.hex()),
                }
            }
            b'>' => {
                self.at += 1;
                if self.peek() == Some(b'>') {
                    self.at += 1;
                }
                Obj::Name(">>".into(), true)
            }
            b'[' | b']' => {
                self.at += 1;
                Obj::Name(if b == b'[' { "[" } else { "]" }.into(), true)
            }
            b'{' => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    match self.peek() {
                        None => return err("syntaxerror: unterminated procedure"),
                        Some(b'}') => {
                            self.at += 1;
                            break;
                        }
                        _ => {
                            if let Some(t) = self.token()? {
                                items.push(t);
                            }
                        }
                    }
                }
                Obj::Array(Rc::new(RefCell::new(items)), true)
            }
            b'}' | b')' => {
                self.at += 1;
                return err("syntaxerror: unmatched bracket");
            }
            b'/' => {
                self.at += 1;
                if self.peek() == Some(b'/') {
                    self.at += 1;
                    // An immediately evaluated name: looked up when run (close enough).
                    Obj::Name(self.word().into(), true)
                } else {
                    Obj::name(&self.word())
                }
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.at += 1;
                    return self.token();
                }
                number(&w).unwrap_or_else(|| Obj::Name(w.into(), true))
            }
        };
        Ok(Some(o))
    }
}

fn number(w: &str) -> Option<Obj> {
    if let Ok(i) = w.parse::<i64>() {
        return Some(Obj::Int(i));
    }
    if let Some((radix, digits)) = w.split_once('#')
        && let Ok(r) = radix.parse::<u32>()
        && (2..=36).contains(&r)
    {
        return i64::from_str_radix(digits, r).ok().map(Obj::Int);
    }
    if w.bytes().next().is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.')) && w.bytes().any(|b| b.is_ascii_digit()) {
        return w.parse::<f64>().ok().filter(|v| v.is_finite()).map(Obj::Real);
    }
    None
}

// ---------------------------------------------------------------- interpreter

#[derive(Clone)]
struct GState {
    ctm: Affine,
    fill: [u8; 3],
    stroke: [u8; 3],
    /// Components the colour space takes (1 grey, 3 RGB, 4 CMYK).
    space: usize,
    path: BezPath,
    current: Option<Point>,
    start: Option<Point>,
    font: Obj,
}

impl Default for GState {
    fn default() -> Self {
        Self { ctm: Affine::IDENTITY, fill: [0; 3], stroke: [0; 3], space: 1, path: BezPath::new(), current: None, start: None, font: Obj::Null }
    }
}

struct Interp {
    lex: Lexer,
    stack: Vec<Obj>,
    dicts: Vec<Dict>,
    systemdict: Dict,
    g: GState,
    saved: Vec<GState>,
    out: Vec<Imported>,
    warnings: Vec<String>,
    ops: u64,
    depth: usize,
}

/// Operators the interpreter knows (each is an entry in `systemdict`).
const OPERATORS: &[&str] = &[
    "pop", "exch", "dup", "copy", "index", "roll", "clear", "count", "mark", "cleartomark", "counttomark", "[", "]", "<<", ">>",
    "add", "sub", "mul", "div", "idiv", "mod", "neg", "abs", "sqrt", "sin", "cos", "atan", "exp", "ln", "log", "round", "floor", "ceiling",
    "truncate", "cvi", "cvr", "rand", "srand", "rrand", "bitshift", "eq", "ne", "gt", "ge", "lt", "le", "and", "or", "not", "xor", "true", "false",
    "null", "if", "ifelse", "for", "repeat", "loop", "exit", "forall", "stop", "stopped", "exec", "quit", "dict", "begin", "end", "def", "load",
    "store", "get", "put", "known", "where", "undef", "currentdict", "countdictstack", "userdict", "systemdict", "globaldict", "statusdict",
    "errordict", "$error", "maxlength", "length", "array", "aload", "astore", "getinterval", "putinterval", "string", "cvs", "cvrs", "cvn",
    "cvx", "cvlit", "xcheck", "rcheck", "wcheck", "type", "bind", "readonly", "executeonly", "noaccess", "search", "anchorsearch", "token",
    "save", "restore", "gsave", "grestore", "grestoreall", "currentglobal", "setglobal", "languagelevel", "version", "product", "revision",
    "serialnumber", "vmstatus", "realtime", "usertime", "flush", "print", "=", "==", "pstack", "stack", "matrix", "identmatrix",
    "currentmatrix", "setmatrix", "defaultmatrix", "initmatrix", "concat", "concatmatrix", "invertmatrix", "translate", "scale", "rotate",
    "transform", "itransform", "dtransform", "idtransform", "initgraphics", "newpath", "moveto", "rmoveto", "lineto", "rlineto", "curveto",
    "rcurveto", "closepath", "arc", "arcn", "arct", "arcto", "currentpoint", "pathbbox", "flattenpath", "reversepath", "strokepath",
    "clippath", "initclip", "clip", "eoclip", "rectclip", "fill", "eofill", "stroke", "rectfill", "rectstroke", "setlinewidth",
    "currentlinewidth", "setlinecap", "setlinejoin", "setmiterlimit", "setdash", "setflat", "setstrokeadjust", "setoverprint",
    "setsmoothness", "setgray", "currentgray", "setrgbcolor", "currentrgbcolor", "setcmykcolor", "currentcmykcolor", "sethsbcolor",
    "setcolorspace", "currentcolorspace", "setcolor", "currentcolor", "setpattern", "sethalftone", "setscreen", "settransfer",
    "setcolortransfer", "setblackgeneration", "setundercolorremoval", "setcolorrendering", "findfont", "definefont", "scalefont",
    "makefont", "setfont", "selectfont", "currentfont", "show", "ashow", "widthshow", "awidthshow", "kshow", "xshow", "yshow", "xyshow",
    "glyphshow", "stringwidth", "charpath", "image", "imagemask", "colorimage", "shfill", "showpage", "copypage", "setpagedevice",
    "currentpagedevice", "erasepage", "findresource", "defineresource", "resourcestatus", "undefineresource", "filter", "currentfile",
    "readhexstring", "readstring", "readline", "closefile", "setpacking", "currentpacking", "setuserparams", "setsystemparams",
    "currentuserparams", "currentsystemparams", "cachestatus", "setcachedevice", "setcharwidth", "max", "min",
];

/// Operators of Illustrator's legacy format and how many operands each takes off the stack when
/// it only sets something BurnCraft doesn't keep.
const AI_IGNORED: &[(&str, usize)] = &[
    ("w", 1), ("j", 1), ("J", 1), ("M", 1), ("d", 2), ("D", 1), ("A", 1), ("O", 1), ("R", 1), ("XR", 1), ("i", 1), ("u", 0), ("U", 0),
    ("*u", 0), ("*U", 0), ("q", 0), ("Q", 0), ("W", 0), ("Lb", 10), ("Ln", 1), ("LB", 0), ("Xw", 1), ("XW", 6), ("Ap", 1), ("Ar", 1),
    ("XT", 1), ("Xb", 1), ("XB", 1), ("Xm", 6), ("Bb", 0), ("BB", 1), ("Bg", 12), ("Bm", 6), ("Bc", 6), ("Bh", 4), ("H", 0), ("N", 0),
];

impl Interp {
    fn new(src: Rc<[u8]>) -> Self {
        let systemdict: Dict = Rc::new(RefCell::new(OPERATORS.iter().map(|n| (Key::from(*n), Obj::Op((*n).into()))).collect()));
        let globaldict: Dict = Default::default();
        let userdict: Dict = Default::default();
        Self {
            lex: Lexer { src, at: 0 },
            stack: Vec::new(),
            dicts: vec![systemdict.clone(), globaldict, userdict],
            systemdict,
            g: GState::default(),
            saved: Vec::new(),
            out: Vec::new(),
            warnings: Vec::new(),
            ops: 0,
            depth: 0,
        }
    }

    fn warn(&mut self, w: &str) {
        if !self.warnings.iter().any(|x| x == w) {
            self.warnings.push(w.to_string());
        }
    }

    // ----- stack helpers

    fn push(&mut self, o: Obj) -> Res {
        if self.stack.len() >= MAX_STACK {
            return err("stackoverflow");
        }
        self.stack.push(o);
        Ok(())
    }
    fn pop(&mut self) -> Res<Obj> {
        self.stack.pop().map_or_else(|| err("stackunderflow"), Ok)
    }
    fn num(&mut self) -> Res<f64> {
        match self.pop()?.num() {
            Some(v) => Ok(v),
            None => err("typecheck: a number was expected"),
        }
    }
    fn nums<const N: usize>(&mut self) -> Res<[f64; N]> {
        let mut v = [0.0; N];
        for i in (0..N).rev() {
            v[i] = self.num()?;
        }
        Ok(v)
    }
    fn int(&mut self) -> Res<i64> {
        Ok(self.num()?.round() as i64)
    }
    fn count(&mut self) -> Res<usize> {
        let n = self.int()?;
        usize::try_from(n).map_or_else(|_| err("rangecheck"), Ok)
    }
    fn boolean(&mut self) -> Res<bool> {
        match self.pop()? {
            Obj::Bool(b) => Ok(b),
            _ => err("typecheck: a boolean was expected"),
        }
    }
    fn proc(&mut self) -> Res<Obj> {
        let o = self.pop()?;
        match &o {
            Obj::Array(..) | Obj::Name(..) | Obj::Op(_) => Ok(o),
            _ => err("typecheck: a procedure was expected"),
        }
    }
    fn array(&mut self) -> Res<Arr> {
        match self.pop()? {
            Obj::Array(a, _) => Ok(a),
            _ => err("typecheck: an array was expected"),
        }
    }
    fn dict(&mut self) -> Res<Dict> {
        match self.pop()? {
            Obj::Dict(d) => Ok(d),
            _ => err("typecheck: a dictionary was expected"),
        }
    }
    fn push_num(&mut self, v: f64) -> Res {
        if v.fract() == 0.0 && v.abs() < 1e15 { self.push(Obj::Int(v as i64)) } else { self.push(Obj::Real(v)) }
    }
    fn push_real(&mut self, v: f64) -> Res {
        self.push(Obj::Real(v))
    }

    fn lookup(&self, k: &str) -> Option<Obj> {
        self.dicts.iter().rev().find_map(|d| d.borrow().get(k).cloned())
    }

    // ----- execution

    fn run(&mut self) -> Res {
        while let Some(t) = self.lex.token()? {
            self.exec_token(t)?;
        }
        Ok(())
    }

    /// A token from the program or a procedure: executable names run, everything else (procedures
    /// too) is pushed.
    fn exec_token(&mut self, t: Obj) -> Res {
        self.ops += 1;
        if self.ops > MAX_OPS {
            return err("the program runs too long");
        }
        match t {
            Obj::Name(n, true) => self.exec_name(&n),
            Obj::Op(n) => self.op(&n),
            o => self.push(o),
        }
    }

    fn exec_name(&mut self, n: &str) -> Res {
        match self.lookup(n) {
            Some(v) => self.call(v),
            None => self.legacy(n),
        }
    }

    /// Runs `o` as `exec` does.
    fn call(&mut self, o: Obj) -> Res {
        match o {
            Obj::Array(a, true) => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    self.depth -= 1;
                    return err("procedures nest too deeply");
                }
                let items: Vec<Obj> = a.borrow().clone();
                let r = items.into_iter().try_for_each(|t| self.exec_token(t));
                self.depth -= 1;
                r
            }
            Obj::Name(n, true) => self.exec_name(&n),
            Obj::Op(n) => self.op(&n),
            o => self.push(o),
        }
    }

    // ----- graphics helpers

    fn user(&self, x: f64, y: f64) -> Point {
        self.g.ctm * Point::new(x, y)
    }
    fn user_current(&self) -> Res<Point> {
        match self.g.current {
            Some(p) => Ok(self.g.ctm.inverse() * p),
            None => err("nocurrentpoint"),
        }
    }
    fn move_to(&mut self, p: Point) {
        self.g.path.move_to(p);
        self.g.current = Some(p);
        self.g.start = Some(p);
    }
    fn line_to(&mut self, p: Point) -> Res {
        if self.g.current.is_none() {
            return err("nocurrentpoint");
        }
        self.g.path.line_to(p);
        self.g.current = Some(p);
        Ok(())
    }
    fn curve_to(&mut self, a: Point, b: Point, c: Point) -> Res {
        if self.g.current.is_none() {
            return err("nocurrentpoint");
        }
        self.g.path.curve_to(a, b, c);
        self.g.current = Some(c);
        Ok(())
    }
    fn close(&mut self) {
        if self.g.current.is_some() {
            self.g.path.close_path();
            self.g.current = self.g.start;
        }
    }

    /// An arc of the circle round (x, y) in user space from angle `a0` to `a1` (degrees),
    /// counterclockwise unless `cw`; joined to the current point by a line.
    fn arc(&mut self, x: f64, y: f64, r: f64, a0: f64, a1: f64, cw: bool) -> Res {
        let (s, mut e) = (a0.to_radians(), a1.to_radians());
        let tau = std::f64::consts::TAU;
        if cw {
            while e > s {
                e -= tau;
            }
        } else {
            while e < s {
                e += tau;
            }
        }
        let arc = kurbo::Arc { center: Point::new(x, y), radii: Vec2::new(r, r), start_angle: s, sweep_angle: e - s, x_rotation: 0.0 };
        let first = self.user(x + r * s.cos(), y + r * s.sin());
        if self.g.current.is_some() {
            self.line_to(first)?;
        } else {
            self.move_to(first);
        }
        let ctm = self.g.ctm;
        for el in arc.append_iter(0.001) {
            if let PathEl::CurveTo(a, b, c) = el {
                self.curve_to(ctm * a, ctm * b, ctm * c)?;
            }
        }
        Ok(())
    }

    /// Keeps the current path as an outline in `color`, then starts a new path.
    fn paint(&mut self, color: [u8; 3], close: bool) {
        let mut path = std::mem::take(&mut self.g.path);
        if close {
            path.close_path();
        }
        self.g.current = None;
        if path.elements().len() < 2 || self.out.len() >= MAX_SHAPES {
            return;
        }
        // Filled then stroked (or the reverse) is one outline.
        if self.out.last().is_some_and(|o| o.path.elements() == path.elements()) {
            return;
        }
        self.out.push(Imported { path, color: Some(color) });
    }

    fn set_color(&mut self, comps: &[f64], fill: bool, stroke: bool) {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let rgb = match comps.len() {
            1 => [c(comps[0]); 3],
            3 => [c(comps[0]), c(comps[1]), c(comps[2])],
            4 => {
                let k = 1.0 - comps[3].clamp(0.0, 1.0);
                [c((1.0 - comps[0]) * k), c((1.0 - comps[1]) * k), c((1.0 - comps[2]) * k)]
            }
            _ => [0; 3],
        };
        if fill {
            self.g.fill = rgb;
        }
        if stroke {
            self.g.stroke = rgb;
        }
    }

    fn matrix_of(a: &Arr) -> Res<Affine> {
        let v: Vec<f64> = a.borrow().iter().filter_map(Obj::num).collect();
        if v.len() != 6 {
            return err("rangecheck: a matrix has six numbers");
        }
        Ok(Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]))
    }
    fn matrix_obj(m: Affine) -> Obj {
        Obj::array(m.as_coeffs().iter().map(|v| Obj::Real(*v)).collect())
    }
    fn set_matrix(a: &Arr, m: Affine) {
        *a.borrow_mut() = m.as_coeffs().iter().map(|v| Obj::Real(*v)).collect();
    }

    /// `translate`, `scale`, `rotate`: on the CTM, or with a matrix operand, into it.
    fn transform_op(&mut self, make: impl Fn(&mut Self) -> Res<Affine>) -> Res {
        if let Some(Obj::Array(a, _)) = self.stack.last().cloned() {
            self.pop()?;
            let m = make(self)?;
            Self::set_matrix(&a, m);
            return self.push(Obj::Array(a, false));
        }
        let m = make(self)?;
        self.g.ctm *= m;
        Ok(())
    }

    /// `transform` and friends: a point or a distance, by the CTM or a given matrix.
    fn point_op(&mut self, inverse: bool, distance: bool) -> Res {
        let m = match self.stack.last() {
            Some(Obj::Array(..)) => {
                let a = self.array()?;
                Self::matrix_of(&a)?
            }
            _ => self.g.ctm,
        };
        let m = if inverse { m.inverse() } else { m };
        let [x, y] = self.nums()?;
        let p = if distance {
            let c = m.as_coeffs();
            Point::new(c[0] * x + c[2] * y, c[1] * x + c[3] * y)
        } else {
            m * Point::new(x, y)
        };
        self.push_real(p.x)?;
        self.push_real(p.y)
    }

    /// Skips the data an image reads from the program (hex, base 85 or raw bytes).
    fn skip_image_data(&mut self, source: Obj, bytes: usize) -> Res {
        match source {
            Obj::Array(..) | Obj::Name(..) => {
                // A procedure that reads its own rows: run it until the image has its data.
                let mut got = 0;
                let mut calls = 0;
                while got < bytes && calls < 1_000_000 {
                    calls += 1;
                    let before = self.stack.len();
                    self.call(source.clone())?;
                    match self.stack.pop() {
                        Some(Obj::Str(s)) if !s.borrow().is_empty() => got += s.borrow().len(),
                        _ => break,
                    }
                    self.stack.truncate(before);
                }
                Ok(())
            }
            Obj::File => {
                // Raw data straight after the operator.
                if self.lex.peek().is_some_and(|b| b.is_ascii_whitespace()) {
                    self.lex.at += 1;
                }
                self.lex.at = (self.lex.at + bytes).min(self.lex.src.len());
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // ----- operators

    fn op(&mut self, n: &str) -> Res {
        match n {
            // stack
            "pop" => self.pop().map(|_| ()),
            "exch" => {
                let (b, a) = (self.pop()?, self.pop()?);
                self.push(b)?;
                self.push(a)
            }
            "dup" => {
                let a = self.pop()?;
                self.push(a.clone())?;
                self.push(a)
            }
            "copy" => match self.stack.last() {
                Some(Obj::Int(_)) => {
                    let n = self.count()?;
                    let len = self.stack.len();
                    if n > len {
                        return err("stackunderflow");
                    }
                    let tail: Vec<Obj> = self.stack[len - n..].to_vec();
                    tail.into_iter().try_for_each(|o| self.push(o))
                }
                _ => {
                    let (dst, src) = (self.pop()?, self.pop()?);
                    match (src, dst) {
                        (Obj::Array(s, _), Obj::Array(d, x)) => {
                            let items = s.borrow().clone();
                            let n = items.len();
                            d.borrow_mut().splice(0..n.min(d.borrow().len()), items);
                            self.push(Obj::Array(d, x))
                        }
                        (Obj::Str(s), Obj::Str(d)) => {
                            let items = s.borrow().clone();
                            let n = items.len();
                            d.borrow_mut().splice(0..n.min(d.borrow().len()), items);
                            self.push(Obj::Str(d))
                        }
                        (Obj::Dict(s), Obj::Dict(d)) => {
                            let items = s.borrow().clone();
                            d.borrow_mut().extend(items);
                            self.push(Obj::Dict(d))
                        }
                        _ => err("typecheck: copy"),
                    }
                }
            },
            "index" => {
                let n = self.count()?;
                let o = self.stack.iter().rev().nth(n).cloned().ok_or(Ps::Error("rangecheck".into()))?;
                self.push(o)
            }
            "roll" => {
                let j = self.int()?;
                let n = self.count()?;
                let len = self.stack.len();
                if n > len {
                    return err("stackunderflow");
                }
                if n > 0 {
                    let part = &mut self.stack[len - n..];
                    let k = j.rem_euclid(n as i64) as usize;
                    part.rotate_right(k);
                }
                Ok(())
            }
            "clear" => {
                self.stack.clear();
                Ok(())
            }
            "count" => self.push(Obj::Int(self.stack.len() as i64)),
            "mark" | "[" | "<<" => self.push(Obj::Mark),
            "cleartomark" => {
                while !matches!(self.pop()?, Obj::Mark) {}
                Ok(())
            }
            "counttomark" => {
                let n = self.stack.iter().rev().position(|o| matches!(o, Obj::Mark)).ok_or(Ps::Error("unmatchedmark".into()))?;
                self.push(Obj::Int(n as i64))
            }
            "]" => {
                let mut items = Vec::new();
                loop {
                    match self.pop()? {
                        Obj::Mark => break,
                        o => items.push(o),
                    }
                }
                items.reverse();
                self.push(Obj::array(items))
            }
            ">>" => {
                let mut items = Vec::new();
                loop {
                    match self.pop()? {
                        Obj::Mark => break,
                        o => items.push(o),
                    }
                }
                items.reverse();
                let d: HashMap<Key, Obj> = items.chunks(2).filter(|p| p.len() == 2).map(|p| (p[0].key(), p[1].clone())).collect();
                self.push(Obj::Dict(Rc::new(RefCell::new(d))))
            }
            // arithmetic
            "add" | "sub" | "mul" => {
                let (b, a) = (self.pop()?, self.pop()?);
                if let (Obj::Int(x), Obj::Int(y)) = (&a, &b) {
                    let r = match n {
                        "add" => x.checked_add(*y),
                        "sub" => x.checked_sub(*y),
                        _ => x.checked_mul(*y),
                    };
                    if let Some(r) = r {
                        return self.push(Obj::Int(r));
                    }
                }
                let (x, y) = (a.num().ok_or(Ps::Error("typecheck".into()))?, b.num().ok_or(Ps::Error("typecheck".into()))?);
                self.push_real(match n {
                    "add" => x + y,
                    "sub" => x - y,
                    _ => x * y,
                })
            }
            "div" => {
                let [a, b] = self.nums()?;
                if b == 0.0 { err("undefinedresult") } else { self.push_real(a / b) }
            }
            "idiv" | "mod" => {
                let (b, a) = (self.int()?, self.int()?);
                if b == 0 {
                    return err("undefinedresult");
                }
                self.push(Obj::Int(if n == "idiv" { a / b } else { a % b }))
            }
            "max" | "min" => {
                let [a, b] = self.nums()?;
                self.push_num(if n == "max" { a.max(b) } else { a.min(b) })
            }
            "neg" | "abs" => {
                let o = self.pop()?;
                match o {
                    Obj::Int(i) => self.push(Obj::Int(if n == "neg" { -i } else { i.abs() })),
                    Obj::Real(r) => self.push_real(if n == "neg" { -r } else { r.abs() }),
                    _ => err("typecheck"),
                }
            }
            "sqrt" | "sin" | "cos" | "exp" | "ln" | "log" => {
                if n == "exp" {
                    let [a, b] = self.nums()?;
                    return self.push_real(a.powf(b));
                }
                let a = self.num()?;
                self.push_real(match n {
                    "sqrt" => a.max(0.0).sqrt(),
                    "sin" => a.to_radians().sin(),
                    "cos" => a.to_radians().cos(),
                    "ln" => a.ln(),
                    _ => a.log10(),
                })
            }
            "atan" => {
                let [y, x] = self.nums()?;
                self.push_real(y.atan2(x).to_degrees().rem_euclid(360.0))
            }
            "round" | "floor" | "ceiling" | "truncate" => {
                let o = self.pop()?;
                match o {
                    Obj::Int(i) => self.push(Obj::Int(i)),
                    Obj::Real(r) => self.push_real(match n {
                        "round" => (r + 0.5).floor(),
                        "floor" => r.floor(),
                        "ceiling" => r.ceil(),
                        _ => r.trunc(),
                    }),
                    _ => err("typecheck"),
                }
            }
            "cvi" => {
                let o = self.pop()?;
                let v = match &o {
                    Obj::Str(_) => number(&o.text()).and_then(|v| v.num()).unwrap_or(0.0),
                    o => o.num().ok_or(Ps::Error("typecheck".into()))?,
                };
                self.push(Obj::Int(v.trunc() as i64))
            }
            "cvr" => {
                let o = self.pop()?;
                let v = match &o {
                    Obj::Str(_) => number(&o.text()).and_then(|v| v.num()).unwrap_or(0.0),
                    o => o.num().ok_or(Ps::Error("typecheck".into()))?,
                };
                self.push_real(v)
            }
            "rand" => self.push(Obj::Int(12345)),
            "srand" => self.pop().map(|_| ()),
            "rrand" => self.push(Obj::Int(0)),
            "bitshift" => {
                let (s, v) = (self.int()?, self.int()?);
                self.push(Obj::Int(if s >= 0 { v.wrapping_shl(s as u32) } else { v.wrapping_shr((-s) as u32) }))
            }
            // relational and logic
            "eq" | "ne" => {
                let (b, a) = (self.pop()?, self.pop()?);
                let same = match (&a, &b) {
                    (x, y) if x.num().is_some() && y.num().is_some() => x.num() == y.num(),
                    (Obj::Name(..) | Obj::Str(_) | Obj::Op(_), Obj::Name(..) | Obj::Str(_) | Obj::Op(_)) => a.text() == b.text(),
                    (Obj::Bool(x), Obj::Bool(y)) => x == y,
                    (Obj::Null, Obj::Null) | (Obj::Mark, Obj::Mark) => true,
                    (Obj::Array(x, _), Obj::Array(y, _)) => Rc::ptr_eq(x, y),
                    (Obj::Dict(x), Obj::Dict(y)) => Rc::ptr_eq(x, y),
                    _ => false,
                };
                self.push(Obj::Bool(same == (n == "eq")))
            }
            "gt" | "ge" | "lt" | "le" => {
                let (b, a) = (self.pop()?, self.pop()?);
                let ord = match (a.num(), b.num()) {
                    (Some(x), Some(y)) => x.partial_cmp(&y),
                    _ => Some(a.text().cmp(&b.text())),
                };
                use std::cmp::Ordering::*;
                let r = matches!((n, ord), ("gt", Some(Greater)) | ("ge", Some(Greater | Equal)) | ("lt", Some(Less)) | ("le", Some(Less | Equal)));
                self.push(Obj::Bool(r))
            }
            "and" | "or" | "xor" => {
                let (b, a) = (self.pop()?, self.pop()?);
                match (a, b) {
                    (Obj::Bool(x), Obj::Bool(y)) => self.push(Obj::Bool(match n {
                        "and" => x && y,
                        "or" => x || y,
                        _ => x != y,
                    })),
                    (Obj::Int(x), Obj::Int(y)) => self.push(Obj::Int(match n {
                        "and" => x & y,
                        "or" => x | y,
                        _ => x ^ y,
                    })),
                    _ => err("typecheck"),
                }
            }
            "not" => match self.pop()? {
                Obj::Bool(b) => self.push(Obj::Bool(!b)),
                Obj::Int(i) => self.push(Obj::Int(!i)),
                _ => err("typecheck"),
            },
            "true" => self.push(Obj::Bool(true)),
            "false" => self.push(Obj::Bool(false)),
            "null" => self.push(Obj::Null),
            // control
            "if" => {
                let p = self.proc()?;
                if self.boolean()? { self.call(p) } else { Ok(()) }
            }
            "ifelse" => {
                let (b, a) = (self.proc()?, self.proc()?);
                if self.boolean()? { self.call(a) } else { self.call(b) }
            }
            "exec" => {
                let o = self.pop()?;
                self.call(o)
            }
            "for" => {
                let p = self.proc()?;
                let (limit, inc, init) = (self.pop()?, self.pop()?, self.pop()?);
                let ints = matches!((&init, &inc), (Obj::Int(_), Obj::Int(_)));
                let (mut v, step, end) = (init.num().unwrap_or(0.0), inc.num().unwrap_or(0.0), limit.num().unwrap_or(0.0));
                if step == 0.0 {
                    return Ok(());
                }
                while (step > 0.0 && v <= end) || (step < 0.0 && v >= end) {
                    if ints { self.push(Obj::Int(v as i64))? } else { self.push_real(v)? }
                    match self.call(p.clone()) {
                        Err(Ps::Exit) => break,
                        r => r?,
                    }
                    v += step;
                }
                Ok(())
            }
            "repeat" => {
                let p = self.proc()?;
                let k = self.count()?;
                for _ in 0..k {
                    match self.call(p.clone()) {
                        Err(Ps::Exit) => break,
                        r => r?,
                    }
                }
                Ok(())
            }
            "loop" => {
                let p = self.proc()?;
                loop {
                    match self.call(p.clone()) {
                        Err(Ps::Exit) => return Ok(()),
                        r => r?,
                    }
                }
            }
            "exit" => Err(Ps::Exit),
            "forall" => {
                let p = self.proc()?;
                let items: Vec<Vec<Obj>> = match self.pop()? {
                    Obj::Array(a, _) => a.borrow().iter().map(|o| vec![o.clone()]).collect(),
                    Obj::Str(s) => s.borrow().iter().map(|b| vec![Obj::Int(*b as i64)]).collect(),
                    Obj::Dict(d) => d.borrow().iter().map(|(k, v)| vec![Obj::name(k), v.clone()]).collect(),
                    _ => return err("typecheck: forall"),
                };
                for item in items {
                    for o in item {
                        self.push(o)?;
                    }
                    match self.call(p.clone()) {
                        Err(Ps::Exit) => break,
                        r => r?,
                    }
                }
                Ok(())
            }
            "stop" => Err(Ps::Stop),
            "stopped" => {
                let p = self.proc()?;
                let depth = self.dicts.len();
                match self.call(p) {
                    Ok(()) => self.push(Obj::Bool(false)),
                    Err(Ps::Stop | Ps::Error(_)) => {
                        self.dicts.truncate(depth.max(3));
                        self.push(Obj::Bool(true))
                    }
                    Err(e) => Err(e),
                }
            }
            "quit" => Err(Ps::Quit),
            // dictionaries
            "dict" => {
                self.pop()?;
                self.push(Obj::Dict(Default::default()))
            }
            "begin" => {
                let d = self.dict()?;
                if self.dicts.len() > 1000 {
                    return err("dictstackoverflow");
                }
                self.dicts.push(d);
                Ok(())
            }
            "end" => {
                if self.dicts.len() > 3 {
                    self.dicts.pop();
                }
                Ok(())
            }
            "def" => {
                let (v, k) = (self.pop()?, self.pop()?);
                if let Some(d) = self.dicts.last() {
                    d.borrow_mut().insert(k.key(), v);
                }
                Ok(())
            }
            "load" => {
                let k = self.pop()?.key();
                match self.lookup(&k) {
                    Some(v) => self.push(v),
                    None => err(format!("undefined: {k}")),
                }
            }
            "store" => {
                let (v, k) = (self.pop()?, self.pop()?.key());
                let d = self.dicts.iter().rev().find(|d| d.borrow().contains_key(&k)).cloned().or_else(|| self.dicts.last().cloned());
                if let Some(d) = d {
                    d.borrow_mut().insert(k, v);
                }
                Ok(())
            }
            "get" => {
                let (k, c) = (self.pop()?, self.pop()?);
                let v = match c {
                    Obj::Dict(d) => d.borrow().get(&k.key()).cloned().ok_or_else(|| Ps::Error(format!("undefined: {}", k.text())))?,
                    Obj::Array(a, _) => a.borrow().get(k.num().unwrap_or(-1.0) as usize).cloned().ok_or(Ps::Error("rangecheck".into()))?,
                    Obj::Str(s) => Obj::Int(*s.borrow().get(k.num().unwrap_or(-1.0) as usize).ok_or(Ps::Error("rangecheck".into()))? as i64),
                    _ => return err("typecheck: get"),
                };
                self.push(v)
            }
            "put" => {
                let (v, k, c) = (self.pop()?, self.pop()?, self.pop()?);
                match c {
                    Obj::Dict(d) => {
                        d.borrow_mut().insert(k.key(), v);
                    }
                    Obj::Array(a, _) => {
                        let i = k.num().unwrap_or(-1.0) as usize;
                        *a.borrow_mut().get_mut(i).ok_or(Ps::Error("rangecheck".into()))? = v;
                    }
                    Obj::Str(s) => {
                        let i = k.num().unwrap_or(-1.0) as usize;
                        *s.borrow_mut().get_mut(i).ok_or(Ps::Error("rangecheck".into()))? = v.num().unwrap_or(0.0) as u8;
                    }
                    _ => return err("typecheck: put"),
                }
                Ok(())
            }
            "known" => {
                let (k, d) = (self.pop()?.key(), self.dict()?);
                let has = d.borrow().contains_key(&k);
                self.push(Obj::Bool(has))
            }
            "where" => {
                let k = self.pop()?.key();
                match self.dicts.iter().rev().find(|d| d.borrow().contains_key(&k)).cloned() {
                    Some(d) => {
                        self.push(Obj::Dict(d))?;
                        self.push(Obj::Bool(true))
                    }
                    None => self.push(Obj::Bool(false)),
                }
            }
            "undef" => {
                let (k, d) = (self.pop()?.key(), self.dict()?);
                d.borrow_mut().remove(&k);
                Ok(())
            }
            "currentdict" => {
                let d = self.dicts.last().cloned().unwrap_or_default();
                self.push(Obj::Dict(d))
            }
            "countdictstack" => self.push(Obj::Int(self.dicts.len() as i64)),
            "userdict" => self.push(Obj::Dict(self.dicts[2].clone())),
            "globaldict" => self.push(Obj::Dict(self.dicts[1].clone())),
            "systemdict" => self.push(Obj::Dict(self.systemdict.clone())),
            "statusdict" | "errordict" | "$error" => {
                let d = self.systemdict.borrow().get(&Key::from(format!("{n}.dict"))).cloned();
                let d = match d {
                    Some(Obj::Dict(d)) => d,
                    _ => {
                        let d: Dict = Default::default();
                        self.systemdict.borrow_mut().insert(format!("{n}.dict").into(), Obj::Dict(d.clone()));
                        d
                    }
                };
                self.push(Obj::Dict(d))
            }
            "maxlength" => {
                self.pop()?;
                self.push(Obj::Int(1000))
            }
            "length" => {
                let l = match self.pop()? {
                    Obj::Array(a, _) => a.borrow().len(),
                    Obj::Str(s) => s.borrow().len(),
                    Obj::Dict(d) => d.borrow().len(),
                    o @ (Obj::Name(..) | Obj::Op(_)) => o.text().len(),
                    _ => return err("typecheck: length"),
                };
                self.push(Obj::Int(l as i64))
            }
            // arrays and strings
            "array" => {
                let k = self.count()?.min(MAX_STACK);
                self.push(Obj::array(vec![Obj::Null; k]))
            }
            "aload" => {
                let a = self.array()?;
                for o in a.borrow().iter() {
                    self.stack.push(o.clone());
                }
                self.push(Obj::Array(a, false))
            }
            "astore" => {
                let a = self.array()?;
                let k = a.borrow().len();
                let len = self.stack.len();
                if k > len {
                    return err("stackunderflow");
                }
                *a.borrow_mut() = self.stack.split_off(len - k);
                self.push(Obj::Array(a, false))
            }
            "getinterval" => {
                let (k, i) = (self.count()?, self.count()?);
                match self.pop()? {
                    Obj::Array(a, x) => {
                        let v = a.borrow().get(i..i + k).ok_or(Ps::Error("rangecheck".into()))?.to_vec();
                        self.push(Obj::Array(Rc::new(RefCell::new(v)), x))
                    }
                    Obj::Str(s) => {
                        let v = s.borrow().get(i..i + k).ok_or(Ps::Error("rangecheck".into()))?.to_vec();
                        self.push(Obj::string(v))
                    }
                    _ => err("typecheck: getinterval"),
                }
            }
            "putinterval" => {
                let (src, i, dst) = (self.pop()?, self.count()?, self.pop()?);
                match (dst, src) {
                    (Obj::Array(d, _), Obj::Array(s, _)) => {
                        let s = s.borrow().clone();
                        let mut d = d.borrow_mut();
                        for (k, o) in s.into_iter().enumerate() {
                            *d.get_mut(i + k).ok_or(Ps::Error("rangecheck".into()))? = o;
                        }
                        Ok(())
                    }
                    (Obj::Str(d), Obj::Str(s)) => {
                        let s = s.borrow().clone();
                        let mut d = d.borrow_mut();
                        for (k, b) in s.into_iter().enumerate() {
                            *d.get_mut(i + k).ok_or(Ps::Error("rangecheck".into()))? = b;
                        }
                        Ok(())
                    }
                    _ => err("typecheck: putinterval"),
                }
            }
            "string" => {
                let k = self.count()?.min(1 << 24);
                self.push(Obj::string(vec![0; k]))
            }
            "cvs" | "cvrs" => {
                let s = match self.pop()? {
                    Obj::Str(s) => s,
                    _ => return err("typecheck: cvs"),
                };
                let text = if n == "cvrs" {
                    let radix = self.int()?;
                    let v = self.int()?;
                    match radix {
                        16 => format!("{v:X}"),
                        8 => format!("{v:o}"),
                        2 => format!("{v:b}"),
                        _ => v.to_string(),
                    }
                } else {
                    self.pop()?.text()
                };
                let bytes = text.into_bytes();
                {
                    let mut b = s.borrow_mut();
                    let k = bytes.len().min(b.len());
                    b[..k].copy_from_slice(&bytes[..k]);
                }
                self.push(Obj::string(bytes))
            }
            "cvn" => {
                let o = self.pop()?;
                self.push(Obj::name(&o.text()))
            }
            "cvx" => {
                let o = match self.pop()? {
                    Obj::Name(k, _) => Obj::Name(k, true),
                    Obj::Array(a, _) => Obj::Array(a, true),
                    o => o,
                };
                self.push(o)
            }
            "cvlit" => {
                let o = match self.pop()? {
                    Obj::Name(k, _) => Obj::Name(k, false),
                    Obj::Array(a, _) => Obj::Array(a, false),
                    o => o,
                };
                self.push(o)
            }
            "xcheck" => {
                let x = matches!(self.pop()?, Obj::Name(_, true) | Obj::Array(_, true) | Obj::Op(_));
                self.push(Obj::Bool(x))
            }
            "rcheck" | "wcheck" => {
                self.pop()?;
                self.push(Obj::Bool(true))
            }
            "type" => {
                let o = self.pop()?;
                self.push(Obj::Name(o.type_name().into(), true))
            }
            "bind" => {
                let o = self.pop()?;
                if let Obj::Array(a, _) = &o {
                    self.bind(a, 0);
                }
                self.push(o)
            }
            "readonly" | "executeonly" | "noaccess" | "setpacking" | "setglobal" | "setuserparams" | "setsystemparams" => {
                if !matches!(n, "readonly" | "executeonly" | "noaccess") {
                    self.pop()?;
                    return Ok(());
                }
                Ok(())
            }
            "search" | "anchorsearch" => {
                let (seek, s) = (self.pop()?.text().into_bytes(), self.pop()?);
                let hay = match &s {
                    Obj::Str(b) => b.borrow().clone(),
                    _ => return err("typecheck: search"),
                };
                let found = if seek.is_empty() {
                    Some(0)
                } else if n == "anchorsearch" {
                    hay.starts_with(&seek).then_some(0)
                } else {
                    hay.windows(seek.len()).position(|w| w == seek.as_slice())
                };
                match found {
                    Some(i) => {
                        self.push(Obj::string(hay[i + seek.len()..].to_vec()))?;
                        self.push(Obj::string(seek.clone()))?;
                        if n == "search" {
                            self.push(Obj::string(hay[..i].to_vec()))?;
                        }
                        self.push(Obj::Bool(true))
                    }
                    None => {
                        self.push(s)?;
                        self.push(Obj::Bool(false))
                    }
                }
            }
            "token" => match self.pop()? {
                Obj::Str(s) => {
                    let bytes: Rc<[u8]> = s.borrow().clone().into();
                    let mut lex = Lexer { src: bytes.clone(), at: 0 };
                    match lex.token()? {
                        Some(t) => {
                            self.push(Obj::string(bytes[lex.at..].to_vec()))?;
                            self.push(t)?;
                            self.push(Obj::Bool(true))
                        }
                        None => self.push(Obj::Bool(false)),
                    }
                }
                Obj::File => match self.lex.token()? {
                    Some(t) => {
                        self.push(t)?;
                        self.push(Obj::Bool(true))
                    }
                    None => self.push(Obj::Bool(false)),
                },
                _ => err("typecheck: token"),
            },
            // VM, environment and output
            "save" => {
                self.saved.push(self.g.clone());
                self.push(Obj::Save)
            }
            "restore" => {
                self.pop()?;
                if let Some(g) = self.saved.pop() {
                    self.g = g;
                }
                Ok(())
            }
            "gsave" => {
                if self.saved.len() < 10_000 {
                    self.saved.push(self.g.clone());
                }
                Ok(())
            }
            "grestore" => {
                if let Some(g) = self.saved.pop() {
                    self.g = g;
                }
                Ok(())
            }
            "grestoreall" => {
                if let Some(g) = self.saved.first().cloned() {
                    self.g = g;
                    self.saved.clear();
                }
                Ok(())
            }
            "currentglobal" | "currentpacking" => self.push(Obj::Bool(false)),
            "languagelevel" => self.push(Obj::Int(3)),
            "version" => self.push(Obj::string(b"3010".to_vec())),
            "product" => self.push(Obj::string(b"BurnCraft".to_vec())),
            "revision" | "serialnumber" | "realtime" | "usertime" => self.push(Obj::Int(0)),
            "vmstatus" => {
                self.push(Obj::Int(0))?;
                self.push(Obj::Int(0))?;
                self.push(Obj::Int(1 << 30))
            }
            "currentuserparams" | "currentsystemparams" | "currentpagedevice" => self.push(Obj::Dict(Default::default())),
            "cachestatus" => (0..7).try_for_each(|_| self.push(Obj::Int(0))),
            "flush" | "pstack" | "stack" => Ok(()),
            "print" | "=" | "==" => self.pop().map(|_| ()),
            // matrices and coordinates
            "matrix" => self.push(Self::matrix_obj(Affine::IDENTITY)),
            "identmatrix" | "defaultmatrix" => {
                let a = self.array()?;
                Self::set_matrix(&a, Affine::IDENTITY);
                self.push(Obj::Array(a, false))
            }
            "currentmatrix" => {
                let a = self.array()?;
                Self::set_matrix(&a, self.g.ctm);
                self.push(Obj::Array(a, false))
            }
            "setmatrix" => {
                let a = self.array()?;
                self.g.ctm = Self::matrix_of(&a)?;
                Ok(())
            }
            "initmatrix" | "initgraphics" => {
                self.g.ctm = Affine::IDENTITY;
                Ok(())
            }
            "concat" => {
                let a = self.array()?;
                self.g.ctm *= Self::matrix_of(&a)?;
                Ok(())
            }
            "concatmatrix" => {
                let (c, b, a) = (self.array()?, self.array()?, self.array()?);
                let m = Self::matrix_of(&b)? * Self::matrix_of(&a)?;
                Self::set_matrix(&c, m);
                self.push(Obj::Array(c, false))
            }
            "invertmatrix" => {
                let (b, a) = (self.array()?, self.array()?);
                Self::set_matrix(&b, Self::matrix_of(&a)?.inverse());
                self.push(Obj::Array(b, false))
            }
            "translate" => self.transform_op(|s| {
                let [x, y] = s.nums()?;
                Ok(Affine::translate((x, y)))
            }),
            "scale" => self.transform_op(|s| {
                let [x, y] = s.nums()?;
                Ok(Affine::scale_non_uniform(x, y))
            }),
            "rotate" => self.transform_op(|s| {
                let a = s.num()?;
                Ok(Affine::rotate(a.to_radians()))
            }),
            "transform" => self.point_op(false, false),
            "itransform" => self.point_op(true, false),
            "dtransform" => self.point_op(false, true),
            "idtransform" => self.point_op(true, true),
            // paths
            "newpath" => {
                self.g.path = BezPath::new();
                self.g.current = None;
                Ok(())
            }
            "moveto" => {
                let [x, y] = self.nums()?;
                let p = self.user(x, y);
                self.move_to(p);
                Ok(())
            }
            "rmoveto" | "rlineto" => {
                let [dx, dy] = self.nums()?;
                let c = self.user_current()?;
                let p = self.user(c.x + dx, c.y + dy);
                if n == "rmoveto" {
                    self.move_to(p);
                    Ok(())
                } else {
                    self.line_to(p)
                }
            }
            "lineto" => {
                let [x, y] = self.nums()?;
                let p = self.user(x, y);
                self.line_to(p)
            }
            "curveto" => {
                let [a, b, c, d, e, f] = self.nums()?;
                let (p1, p2, p3) = (self.user(a, b), self.user(c, d), self.user(e, f));
                self.curve_to(p1, p2, p3)
            }
            "rcurveto" => {
                let [a, b, c, d, e, f] = self.nums()?;
                let o = self.user_current()?;
                let (p1, p2, p3) = (self.user(o.x + a, o.y + b), self.user(o.x + c, o.y + d), self.user(o.x + e, o.y + f));
                self.curve_to(p1, p2, p3)
            }
            "closepath" => {
                self.close();
                Ok(())
            }
            "arc" | "arcn" => {
                let [x, y, r, a0, a1] = self.nums()?;
                self.arc(x, y, r, a0, a1, n == "arcn")
            }
            "arct" | "arcto" => {
                let [x1, y1, x2, y2, r] = self.nums()?;
                let p0 = self.user_current()?;
                let (p1, p2) = (Point::new(x1, y1), Point::new(x2, y2));
                let (u, v) = ((p0 - p1).normalize(), (p2 - p1).normalize());
                let half = u.dot(v).clamp(-1.0, 1.0).acos() / 2.0;
                let (t1, t2) = if half.tan().abs() < 1e-9 || !half.is_finite() {
                    (p1, p1)
                } else {
                    let d = r / half.tan();
                    (p1 + u * d, p1 + v * d)
                };
                let (a, b) = (self.user(t1.x, t1.y), self.user(t2.x, t2.y));
                self.line_to(a)?;
                if t1 != t2 {
                    // A circular arc between the tangent points, as one cubic.
                    let k = 4.0 / 3.0 * ((std::f64::consts::PI - 2.0 * half) / 4.0).tan();
                    let (c1, c2) = (t1 + (p1 - t1) * k * (r / (p1 - t1).hypot().max(1e-12)), t2 + (p1 - t2) * k * (r / (p1 - t2).hypot().max(1e-12)));
                    let (c1, c2) = (self.user(c1.x, c1.y), self.user(c2.x, c2.y));
                    self.curve_to(c1, c2, b)?;
                }
                if n == "arcto" {
                    for v in [t1.x, t1.y, t2.x, t2.y] {
                        self.push_real(v)?;
                    }
                }
                Ok(())
            }
            "currentpoint" => {
                let p = self.user_current()?;
                self.push_real(p.x)?;
                self.push_real(p.y)
            }
            "pathbbox" => {
                let b = bc_geom::bounds(&(self.g.ctm.inverse() * self.g.path.clone())).ok_or(Ps::Error("nocurrentpoint".into()))?;
                for v in [b.x0, b.y0, b.x1, b.y1] {
                    self.push_real(v)?;
                }
                Ok(())
            }
            "flattenpath" | "reversepath" | "strokepath" | "initclip" | "setstrokeadjust" | "erasepage" | "copypage" | "closefile" => {
                if n == "closefile" {
                    self.pop()?;
                }
                Ok(())
            }
            "clippath" => {
                self.g.path = BezPath::new();
                self.g.current = None;
                Ok(())
            }
            "clip" | "eoclip" => {
                self.warn("clipping in the file was ignored: art it hides comes in too");
                Ok(())
            }
            "rectclip" => {
                match self.stack.last() {
                    Some(Obj::Array(..)) => {
                        self.pop()?;
                    }
                    _ => {
                        self.nums::<4>()?;
                    }
                }
                Ok(())
            }
            "fill" | "eofill" => {
                let c = self.g.fill;
                self.paint(c, false);
                Ok(())
            }
            "stroke" => {
                let c = self.g.stroke;
                self.paint(c, false);
                Ok(())
            }
            "rectfill" | "rectstroke" => {
                if n == "rectstroke" && matches!(self.stack.last(), Some(Obj::Array(..))) && matches!(self.stack.iter().rev().nth(1), Some(Obj::Array(..))) {
                    self.pop()?; // a matrix
                }
                let rects: Vec<f64> = match self.stack.last() {
                    Some(Obj::Array(..)) => self.array()?.borrow().iter().filter_map(Obj::num).collect(),
                    _ => self.nums::<4>()?.to_vec(),
                };
                let saved = (std::mem::take(&mut self.g.path), self.g.current);
                for r in rects.chunks(4).filter(|c| c.len() == 4) {
                    self.move_to(self.user(r[0], r[1]));
                    for (x, y) in [(r[0] + r[2], r[1]), (r[0] + r[2], r[1] + r[3]), (r[0], r[1] + r[3])] {
                        let p = self.user(x, y);
                        self.line_to(p)?;
                    }
                    self.close();
                }
                let c = if n == "rectfill" { self.g.fill } else { self.g.stroke };
                self.paint(c, false);
                (self.g.path, self.g.current) = saved;
                Ok(())
            }
            "setlinewidth" | "setlinecap" | "setlinejoin" | "setmiterlimit" | "setflat" | "setoverprint" | "setsmoothness" | "settransfer"
            | "setblackgeneration" | "setundercolorremoval" | "setcolorrendering" | "setpagedevice" | "setcharwidth" => {
                self.pop()?;
                if n == "setcharwidth" {
                    self.pop()?;
                }
                Ok(())
            }
            "setcachedevice" => (0..6).try_for_each(|_| self.pop().map(|_| ())),
            "setcolortransfer" => (0..4).try_for_each(|_| self.pop().map(|_| ())),
            "setscreen" => (0..3).try_for_each(|_| self.pop().map(|_| ())),
            "sethalftone" => self.pop().map(|_| ()),
            "setdash" => {
                self.pop()?;
                self.pop().map(|_| ())
            }
            "currentlinewidth" => self.push_real(1.0),
            // colour
            "setgray" => {
                let g = self.num()?;
                self.g.space = 1;
                self.set_color(&[g], true, true);
                Ok(())
            }
            "setrgbcolor" => {
                let c = self.nums::<3>()?;
                self.g.space = 3;
                self.set_color(&c, true, true);
                Ok(())
            }
            "setcmykcolor" => {
                let c = self.nums::<4>()?;
                self.g.space = 4;
                self.set_color(&c, true, true);
                Ok(())
            }
            "sethsbcolor" => {
                let [h, s, v] = self.nums()?;
                let i = (h * 6.0).floor();
                let f = h * 6.0 - i;
                let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
                let rgb = match i as i64 % 6 {
                    0 => [v, t, p],
                    1 => [q, v, p],
                    2 => [p, v, t],
                    3 => [p, q, v],
                    4 => [t, p, v],
                    _ => [v, p, q],
                };
                self.set_color(&rgb, true, true);
                Ok(())
            }
            "setcolorspace" => {
                let o = self.pop()?;
                let name = match &o {
                    Obj::Array(a, _) => a.borrow().first().map(Obj::text).unwrap_or_default(),
                    o => o.text(),
                };
                self.g.space = match name.as_str() {
                    "DeviceRGB" | "CalRGB" => 3,
                    "DeviceCMYK" => 4,
                    _ => 1,
                };
                Ok(())
            }
            "setcolor" => {
                let mut comps = Vec::new();
                while comps.len() < self.g.space {
                    match self.stack.last().and_then(Obj::num) {
                        Some(v) => {
                            self.pop()?;
                            comps.push(v);
                        }
                        None => {
                            self.pop()?; // a pattern
                            break;
                        }
                    }
                }
                comps.reverse();
                if comps.len() == self.g.space {
                    self.set_color(&comps, true, true);
                }
                Ok(())
            }
            "setpattern" => {
                self.pop()?;
                self.set_color(&[0.5], true, true);
                Ok(())
            }
            "currentgray" => {
                let [r, g, b] = self.g.fill;
                self.push_real((0.3 * r as f64 + 0.59 * g as f64 + 0.11 * b as f64) / 255.0)
            }
            "currentrgbcolor" | "currentcmykcolor" | "currentcolor" => {
                let [r, g, b] = self.g.fill.map(|v| v as f64 / 255.0);
                if n == "currentcmykcolor" {
                    let k = 1.0 - r.max(g).max(b);
                    let d = (1.0 - k).max(1e-9);
                    for v in [(1.0 - r - k) / d, (1.0 - g - k) / d, (1.0 - b - k) / d, k] {
                        self.push_real(v)?;
                    }
                    Ok(())
                } else {
                    [r, g, b].into_iter().try_for_each(|v| self.push_real(v))
                }
            }
            "currentcolorspace" => self.push(Obj::array(vec![Obj::name(["", "DeviceGray", "", "DeviceRGB", "DeviceCMYK"][self.g.space.min(4)])])),
            // fonts and text (not drawn)
            "findfont" => {
                let name = self.pop()?;
                let d: HashMap<Key, Obj> = [(Key::from("FontName"), name), (Key::from("FontMatrix"), Self::matrix_obj(Affine::scale(0.001))), (Key::from("FontType"), Obj::Int(1))].into_iter().collect();
                self.push(Obj::Dict(Rc::new(RefCell::new(d))))
            }
            "definefont" => {
                let f = self.pop()?;
                self.pop()?;
                self.push(f)
            }
            "scalefont" | "makefont" => {
                self.pop()?;
                Ok(())
            }
            "setfont" => {
                self.g.font = self.pop()?;
                Ok(())
            }
            "selectfont" => {
                self.pop()?;
                self.pop().map(|_| ())
            }
            "currentfont" => {
                let f = match &self.g.font {
                    Obj::Null => Obj::Dict(Default::default()),
                    f => f.clone(),
                };
                self.push(f)
            }
            "show" | "ashow" | "widthshow" | "awidthshow" | "kshow" | "xshow" | "yshow" | "xyshow" | "glyphshow" | "charpath" => {
                let k = match n {
                    "ashow" => 3,
                    "widthshow" => 4,
                    "awidthshow" => 6,
                    "kshow" | "xshow" | "yshow" | "xyshow" | "charpath" => 2,
                    _ => 1,
                };
                for _ in 0..k {
                    self.pop()?;
                }
                self.warn("text in the file was left out (convert it to outlines in the program that made it)");
                Ok(())
            }
            "stringwidth" => {
                let s = self.pop()?.text();
                self.push_real(s.len() as f64 * 0.5)?;
                self.push_real(0.0)
            }
            // pictures (not drawn): their data is skipped
            "image" | "imagemask" | "colorimage" => {
                self.warn("pictures in the file were left out (import them separately to engrave them)");
                let (w, h, bpc, comps, source) = match self.stack.last() {
                    Some(Obj::Dict(_)) => {
                        let d = self.dict()?;
                        let d = d.borrow();
                        let get = |k: &str| d.get(k).and_then(Obj::num).unwrap_or(0.0) as usize;
                        let comps = d.get("Decode").map(|o| if let Obj::Array(a, _) = o { a.borrow().len() / 2 } else { 1 }).unwrap_or(1);
                        (get("Width"), get("Height"), get("BitsPerComponent").max(1), comps.max(1), d.get("DataSource").cloned().unwrap_or(Obj::Null))
                    }
                    _ if n == "colorimage" => {
                        let ncomp = self.count()?;
                        let multi = self.boolean()?;
                        let mut sources = Vec::new();
                        for _ in 0..if multi { ncomp } else { 1 } {
                            sources.push(self.pop()?);
                        }
                        self.pop()?; // matrix
                        let bpc = self.count()?;
                        let [w, h] = self.nums()?;
                        (w as usize, h as usize, bpc.max(1), if multi { 1 } else { ncomp }, sources.pop().unwrap_or(Obj::Null))
                    }
                    _ => {
                        let source = self.pop()?;
                        self.pop()?; // matrix
                        let bpc = self.pop()?;
                        let [w, h] = self.nums()?;
                        let bpc = if n == "imagemask" { 1 } else { bpc.num().unwrap_or(8.0) as usize };
                        (w as usize, h as usize, bpc.max(1), 1, source)
                    }
                };
                let bytes = (w * bpc * comps).div_ceil(8).saturating_mul(h);
                self.skip_image_data(source, bytes)
            }
            "shfill" => self.pop().map(|_| ()),
            "showpage" => Err(Ps::Quit),
            // resources and files
            "findresource" => {
                let (cat, key) = (self.pop()?.text(), self.pop()?.key());
                let res = self.systemdict.borrow().get(&Key::from(format!("resource.{cat}.{key}"))).cloned();
                match res {
                    Some(v) => self.push(v),
                    None if cat == "ProcSet" => err(format!("undefinedresource: {key}")),
                    None => self.push(Obj::Dict(Default::default())),
                }
            }
            "defineresource" => {
                let (cat, inst, key) = (self.pop()?.text(), self.pop()?, self.pop()?.key());
                self.systemdict.borrow_mut().insert(format!("resource.{cat}.{key}").into(), inst.clone());
                self.push(inst)
            }
            "resourcestatus" => {
                let (cat, key) = (self.pop()?.text(), self.pop()?.key());
                let has = self.systemdict.borrow().contains_key(&Key::from(format!("resource.{cat}.{key}")));
                if has {
                    self.push(Obj::Int(0))?;
                    self.push(Obj::Int(0))?;
                }
                self.push(Obj::Bool(has))
            }
            "undefineresource" => {
                self.pop()?;
                self.pop().map(|_| ())
            }
            "filter" => {
                // `source [params] /Name filter`: data read from the program; images skip it.
                let name = self.pop()?.text();
                if matches!(self.stack.last(), Some(Obj::Dict(_))) {
                    self.pop()?;
                }
                let source = self.pop()?;
                if matches!(source, Obj::File) {
                    if name == "ASCIIHexDecode" {
                        self.lex.hex();
                    } else if name == "ASCII85Decode" {
                        self.lex.base85();
                    }
                }
                self.push(Obj::File)
            }
            "currentfile" => self.push(Obj::File),
            "readhexstring" | "readstring" | "readline" => {
                let s = match self.pop()? {
                    Obj::Str(s) => s,
                    _ => return err("typecheck: read"),
                };
                self.pop()?; // the file
                let want = s.borrow().len();
                let mut got = Vec::with_capacity(want);
                match n {
                    "readhexstring" => {
                        let mut half: Option<u8> = None;
                        while got.len() < want {
                            let Some(b) = self.lex.peek() else { break };
                            self.lex.at += 1;
                            if let Some(d) = (b as char).to_digit(16) {
                                match half.take() {
                                    Some(h) => got.push(h << 4 | d as u8),
                                    None => half = Some(d as u8),
                                }
                            }
                        }
                    }
                    "readstring" => {
                        if self.lex.peek().is_some_and(|b| b.is_ascii_whitespace()) && got.is_empty() {
                            self.lex.at += 1;
                        }
                        let end = (self.lex.at + want).min(self.lex.src.len());
                        got.extend_from_slice(&self.lex.src[self.lex.at..end]);
                        self.lex.at = end;
                    }
                    _ => {
                        while let Some(b) = self.lex.peek() {
                            self.lex.at += 1;
                            if b == b'\n' {
                                break;
                            }
                            got.push(b);
                        }
                    }
                }
                let full = got.len() == want || n == "readline";
                self.push(Obj::string(got))?;
                self.push(Obj::Bool(full))
            }
            other => err(format!("undefined: {other}")),
        }
    }

    /// `bind`: names in `a` (and its procedures) that are operators become the operators.
    fn bind(&self, a: &Arr, depth: usize) {
        if depth > 32 {
            return;
        }
        let mut items = a.borrow_mut();
        for o in items.iter_mut() {
            match o {
                Obj::Name(k, true) => {
                    if let Some(Obj::Op(op)) = self.lookup(k) {
                        *o = Obj::Op(op);
                    }
                }
                Obj::Array(inner, true) if !Rc::ptr_eq(inner, a) => self.bind(inner, depth + 1),
                _ => {}
            }
        }
    }

    /// A name nothing defines: the drawing operators of Illustrator's legacy format (a file that
    /// names a prolog it doesn't include), else an error.
    fn legacy(&mut self, n: &str) -> Res {
        match n {
            "m" => {
                let [x, y] = self.nums()?;
                let p = self.user(x, y);
                self.move_to(p);
                Ok(())
            }
            "l" | "L" => {
                let [x, y] = self.nums()?;
                let p = self.user(x, y);
                self.line_to(p)
            }
            "c" | "C" => {
                let [a, b, c, d, e, f] = self.nums()?;
                let (p1, p2, p3) = (self.user(a, b), self.user(c, d), self.user(e, f));
                self.curve_to(p1, p2, p3)
            }
            "v" | "V" => {
                let [c, d, e, f] = self.nums()?;
                let p1 = self.g.current.ok_or(Ps::Error("nocurrentpoint".into()))?;
                let (p2, p3) = (self.user(c, d), self.user(e, f));
                self.curve_to(p1, p2, p3)
            }
            "y" | "Y" => {
                let [a, b, e, f] = self.nums()?;
                let (p1, p3) = (self.user(a, b), self.user(e, f));
                self.curve_to(p1, p3, p3)
            }
            "h" => {
                self.close();
                Ok(())
            }
            "f" | "F" | "s" | "S" | "b" | "B" | "n" => {
                let close = matches!(n, "f" | "s" | "b" | "n");
                if n == "n" {
                    self.close();
                    self.g.path = BezPath::new();
                    self.g.current = None;
                    return Ok(());
                }
                let c = if matches!(n, "f" | "F") { self.g.fill } else { self.g.stroke };
                self.paint(c, close);
                Ok(())
            }
            "g" | "G" => {
                let g = self.num()?;
                self.set_color(&[g], n == "g", n == "G");
                Ok(())
            }
            "k" | "K" => {
                let c = self.nums::<4>()?;
                self.set_color(&c, n == "k", n == "K");
                Ok(())
            }
            "x" | "X" => {
                // c m y k (name) tint
                let tint = self.num()?;
                self.pop()?;
                let [c, m, y, k] = self.nums()?;
                let t = 1.0 - tint.clamp(0.0, 1.0);
                self.set_color(&[c * t, m * t, y * t, k * t], n == "x", n == "X");
                Ok(())
            }
            "Xa" | "XA" => {
                let c = self.nums::<3>()?;
                self.set_color(&c, n == "Xa", n == "XA");
                Ok(())
            }
            "Xx" | "XX" => {
                // c m y k (name) tint type (or r g b …)
                let kind = self.int()?;
                let tint = self.num()?;
                self.pop()?;
                let t = 1.0 - tint.clamp(0.0, 1.0);
                if kind == 1 {
                    let [r, g, b] = self.nums()?;
                    self.set_color(&[1.0 - (1.0 - r) * t, 1.0 - (1.0 - g) * t, 1.0 - (1.0 - b) * t], n == "Xx", n == "XX");
                } else {
                    let [c, m, y, k] = self.nums()?;
                    self.set_color(&[c * t, m * t, y * t, k * t], n == "Xx", n == "XX");
                }
                Ok(())
            }
            _ => match AI_IGNORED.iter().find(|(k, _)| *k == n) {
                Some((_, count)) => (0..*count).try_for_each(|_| self.pop().map(|_| ())),
                None => err(format!("undefined: {n}")),
            },
        }
    }
}

/// The PostScript program in `bytes`: a DOS EPS binary header points to it.
fn program(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) && bytes.len() >= 12 {
        let at = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        if let Some(p) = bytes.get(at..at.saturating_add(len)) {
            return p;
        }
    }
    bytes
}

/// The page from the header comments, in points.
fn bounding_box(src: &[u8]) -> Option<Rect> {
    let text = String::from_utf8_lossy(&src[..src.len().min(64 * 1024)]);
    let find = |key: &str| {
        text.lines().find_map(|l| {
            let rest = l.strip_prefix(key)?;
            let v: Vec<f64> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            (v.len() == 4 && v[2] > v[0] && v[3] > v[1]).then(|| Rect::new(v[0], v[1], v[2], v[3]))
        })
    };
    find("%%HiResBoundingBox:").or_else(|| find("%%BoundingBox:"))
}

/// Reads an EPS or PostScript file: what it draws, in millimetres from the page's top-left.
pub fn import(bytes: &[u8]) -> Result<ImportResult, String> {
    let src = program(bytes);
    if !src.starts_with(b"%!") && !bytes.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) {
        return Err("not a PostScript or EPS file".into());
    }
    let page = bounding_box(src);
    let mut it = Interp::new(src.into());
    let stopped = match it.run() {
        Ok(()) | Err(Ps::Quit) => None,
        Err(Ps::Error(e)) => Some(e),
        Err(Ps::Stop) => Some("the program stopped".into()),
        Err(Ps::Exit) => None,
    };
    let mut warnings = std::mem::take(&mut it.warnings);
    if it.out.is_empty() {
        return Err(match stopped {
            Some(e) => format!("the file's PostScript couldn't be read ({e})"),
            None => "the file draws no outlines".into(),
        });
    }
    if let Some(e) = stopped {
        warnings.insert(0, format!("the file's PostScript stopped early ({e}): what it drew before that was imported"));
    }
    let page = page.or_else(|| it.out.iter().filter_map(|s| bc_geom::bounds(&s.path)).reduce(|a, b| a.union(b))).unwrap_or_default();
    // Points, y up → millimetres, y down from the page's top-left.
    let to_mm = Affine::new([MM_PER_PT, 0.0, 0.0, -MM_PER_PT, -page.x0 * MM_PER_PT, page.y1 * MM_PER_PT]);
    let shapes = it.out.into_iter().map(|s| Imported { path: to_mm * s.path, color: s.color }).collect();
    Ok(ImportResult { shapes, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ps: &str) -> ImportResult {
        import(ps.as_bytes()).unwrap()
    }

    #[test]
    fn a_square_comes_in_at_its_size_from_the_top_left() {
        let r = run("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 144 144\n1 0 0 setrgbcolor\n36 36 moveto 72 0 rlineto 0 72 rlineto -72 0 rlineto closepath fill\n");
        assert_eq!(r.shapes.len(), 1);
        assert_eq!(r.shapes[0].color, Some([255, 0, 0]));
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        assert!((b.x0 - 12.7).abs() < 1e-6 && (b.width() - 25.4).abs() < 1e-6 && (b.y0 - 12.7).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn procedures_loops_dictionaries_and_transforms_run() {
        let r = run("%!PS\n%%BoundingBox: 0 0 200 200\n/sq { newpath 0 0 moveto 10 0 lineto 10 10 lineto 0 10 lineto closepath stroke } bind def\n\
             10 dict begin /n 4 def end\n0 1 3 { gsave 40 mul 0 translate sq grestore } for\n\
             gsave 100 100 translate 45 rotate 2 2 scale sq grestore\n/x 5 def x 5 eq { 0 0 moveto 1 1 lineto stroke } if\n");
        assert_eq!(r.shapes.len(), 6, "four squares, a turned one and a line: {:?}", r.warnings);
        let turned = bc_geom::bounds(&r.shapes[4].path).unwrap();
        assert!((turned.width() - 20.0 * 2f64.sqrt() * MM_PER_PT).abs() < 0.01, "{turned:?}");
    }

    #[test]
    fn arcs_curves_and_stopped_work() {
        let r = run("%!PS\n%%BoundingBox: 0 0 100 100\n{ undefinedthing } stopped pop\n50 50 25 0 360 arc closepath fill\n");
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        assert!((b.width() - 50.0 * MM_PER_PT).abs() < 0.01 && (b.height() - 50.0 * MM_PER_PT).abs() < 0.01, "{b:?}");
    }

    #[test]
    fn illustrator_legacy_operators_without_their_prolog() {
        let r = run("%!PS-Adobe-2.0 EPSF-1.2\n%%BoundingBox: 0 0 100 100\n%%Creator: Adobe Illustrator(TM) 3.2\n\
             0 0 1 0 k\n10 10 m 90 10 L 90 90 L 10 90 L f\n0 G 20 20 m 80 80 l S\n");
        assert_eq!(r.shapes.len(), 2);
        assert_eq!(r.shapes[0].color, Some([255, 255, 0]), "yellow (CMYK 0 0 1 0)");
    }

    #[test]
    fn an_error_keeps_what_was_drawn() {
        let r = run("%!PS\n%%BoundingBox: 0 0 100 100\n0 0 moveto 10 10 lineto stroke\nnotanoperator\n20 20 moveto 30 30 lineto stroke\n");
        assert_eq!(r.shapes.len(), 1);
        assert!(r.warnings[0].contains("notanoperator"), "{:?}", r.warnings);
        assert!(import(b"GIF89a").is_err());
    }

    #[test]
    fn inline_image_data_is_skipped() {
        let r = run("%!PS\n%%BoundingBox: 0 0 100 100\n/picstr 2 string def\n2 2 8 [2 0 0 2 0 0] {currentfile picstr readhexstring pop} image\nFFFF\n0000\n0 0 moveto 10 10 lineto stroke\n");
        assert_eq!(r.shapes.len(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("pictures")));
    }
}
