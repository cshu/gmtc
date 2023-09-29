#![allow(clippy::print_literal)]
#![allow(clippy::needless_return)]
#![allow(dropping_references)]
#![allow(clippy::assertions_on_constants)]
mod util;

use crabrs::*;
use crabsqliters::*;

use log::*;

use std::path::PathBuf;
use std::process::*;
use std::*;

#[macro_use(defer)]
extern crate scopeguard;

//note bookmark is not stored in sqlite, but in separate file. Because bookmark might be written quite frequently. (E.g. maybe each time user navigates)
//note recent file list is stored in sqlite so it can support huge number of files sorted by last open time.

//note correctness is sacrificed for better performance (not checking grapheme clusters)
//note it does not read text files as grapheme clusters, in other words, sometimes a group of chars are displayed and it might not end at boundary of grapheme cluster

//note there are 2 ways of displaying file content: PAGE-LIKE vs SLIDING-WINDOW

//PAGE-LIKE
//note in order to achieve page-like display effect (similar to PgDn), there is a concept of DISPLAY LINE (like display line in vim). Meaning not a real line in file, but a line occupying one line on terminal.
//`` empty input means next page
//` ` one space input means prev page
//`+` one plus input is the same as ` `
const DEF_DISPLAY_LINE_WIDTH: usize = 64;
const MIN_DISPLAY_LINE_WIDTH: usize = 20; //20 is enough for displaying greatest u64, so enough for displaying line number
const _: () = assert!(
    DEF_DISPLAY_LINE_WIDTH >= MIN_DISPLAY_LINE_WIDTH,
    "Constraint on const"
);
const DEF_DISPLAY_HEIGHT: usize = 8;
const MIN_DISPLAY_HEIGHT: usize = 4;
const _: () = assert!(
    DEF_DISPLAY_HEIGHT >= MIN_DISPLAY_HEIGHT,
    "Constraint on const"
);

//SLIDING-WINDOW
//w move up one line
//s move down one line
//a move prev (move to prev window bytes) (if already at first byte then do nothing)
//d move next (move to next window bytes) (if already at last byte then do nothing)
//f for printing details about the current file and current caret
//{number}| for jumping to column (WINDOW starts at)
//:{number} for jumping to line number
//{number}% for jumping to %
//slash (/) for searching
//{number} for jumping to a certain search result
//se/set for setting options, e.g. se regex/se regex!/se noregex for toggling searching mode, se windowsize {number} for WINDOW size
//e for reloading the file
//e ++enc=<encoding> for reloading the file with encoding
//v for selecting mode (and then use w/s/a/d to move around and press y to copy to clipboard (calling xclip or customized command). Or just press enter with empty input for printing on stdout. Or just input `tee` for writing to a file.)

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const _: () = assert!(!PKG_NAME.is_empty(), "Constraint on const");

const DEF_WIND_SIZE: usize = 1024;
const MIN_WIND_SIZE: usize = 16;
const DEF_CACHE_SIZE: usize = 64;
const MIN_CACHE_SIZE: usize = 3;
const _: () = assert!(DEF_WIND_SIZE >= MIN_WIND_SIZE, "Constraint on const");
const _: () = assert!(DEF_CACHE_SIZE >= MIN_CACHE_SIZE, "Constraint on const");

const DEF_OLDFILES_LST_LEN: usize = 20; //todo make this configurable

fn main() -> ExitCode {
    env::set_var("RUST_BACKTRACE", "1"); //? not 100% sure this has 0 impact on performance? Maybe setting via command line instead of hardcoding is better?
                                         //env::set_var("RUST_LIB_BACKTRACE", "1");//? this line is useless?
                                         ////
    env::set_var("RUST_LOG", "trace"); //note this line must be above logger init.
    env_logger::init();

    let args: Vec<String> = env::args().collect(); //Note that std::env::args will panic if any argument contains invalid Unicode.
    fn the_end() {
        if std::thread::panicking() {
            info!("{}", "PANICKING");
        }
        info!("{}", "FINISHED");
    }
    defer! {
        the_end();
    }
    if main_inner(args).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(0)
}
//use const generics here
fn must_be_ge_otherwise_err<const N: usize>(newval: usize, msg: &'static str) -> CustRes<usize> {
    if newval >= N {
        return Ok(newval);
    }
    dummy_err(msg)
}
fn main_inner(args: Vec<String>) -> CustRes<()> {
    //use rusqlite::Connection;
    use sha2::Digest;
    //let conn = Connection::open_in_memory()?;

    let mut ctx = Ctx {
        hasher: sha2::Sha256::new(),
        args,
        tr: Box::new(UTF8Rdr {}),
        enc: encoding_rs::UTF_8,
        def: CtxDef::default(),
    };
    ctx.def_dlwidth = match env::var("GMTC_DEF_DISPLAY_LINE_WIDTH") {
        Ok(vstr) => must_be_ge_otherwise_err::<MIN_DISPLAY_LINE_WIDTH>(
            vstr.parse()?,
            "GMTC_DEF_DISPLAY_LINE_WIDTH is too small",
        )?,
        Err(_) => DEF_DISPLAY_LINE_WIDTH,
    };
    ctx.def_dheight = match env::var("GMTC_DEF_DISPLAY_HEIGHT") {
        Ok(vstr) => must_be_ge_otherwise_err::<MIN_DISPLAY_HEIGHT>(
            vstr.parse()?,
            "GMTC_DEF_DISPLAY_HEIGHT is too small",
        )?,
        Err(_) => DEF_DISPLAY_HEIGHT,
    };
    ctx.def_wind_size = match env::var("GMTC_DEF_WIND_SIZE") {
        Ok(vstr) => must_be_ge_otherwise_err::<MIN_WIND_SIZE>(
            vstr.parse()?,
            "GMTC_DEF_WIND_SIZE is too small",
        )?,
        Err(_) => DEF_WIND_SIZE,
    };
    ctx.def_cache_size = match env::var("GMTC_DEF_CACHE_SIZE") {
        Ok(vstr) => must_be_ge_otherwise_err::<MIN_CACHE_SIZE>(
            vstr.parse()?,
            "GMTC_DEF_CACHE_SIZE is too small",
        )?,
        Err(_) => DEF_CACHE_SIZE,
    };
    ctx.def_enc_scheme = match env::var("GMTC_DEF_ENCODING_SCHEME") {
        Ok(vstr) => vstr,
        Err(_) => "utf-8".into(),
    };
    match ctx.def.def_enc_scheme.as_str() {
        "utf-8" | "UTF-8" => {}
        "gb18030" | "GB18030" => {
            ctx.tr = Box::new(GB18030Rdr {});
            ctx.enc = encoding_rs::GB18030;
        }
        _ => {
            return dummy_err("Encoding scheme not supported");
        }
    }
    ctx.def.home_dir = dirs::home_dir().ok_or("Failed to get home directory.")?;
    if !real_dir_without_symlink(&ctx.def.home_dir) {
        return dummy_err("Failed to recognize the home dir as folder.");
    }
    ctx.def.everycom = ctx.def.home_dir.join(".everycom");
    ctx.def.app_support_dir = ctx.def.everycom.join(PKG_NAME);
    ctx.def.db_p = ctx.def.app_support_dir.join("db");
    ctx.def.lock_p = ctx.def.app_support_dir.join("lock");
    ctx.def.bookmark_dir = ctx.def.app_support_dir.join("bookmark");
    fs::create_dir_all(&ctx.def.bookmark_dir)?;
    if env::var("GMTC_DEL_RECORDS_OF_NONEXISTENT_FILES") == Ok("true".to_owned()) {
        let mut ok: bool = false;
        del_records_of_nonexistent_files(&mut ctx, &mut ok)?;
        if !ok {
            return Err(CustomErr {});
        }
    }
    if let Ok(del_where) = env::var("GMTC_DEL_RECORDS_WHERE") {
        let mut ok: bool = false;
        del_records_where(&mut ctx, &mut ok, del_where)?;
        if !ok {
            return Err(CustomErr {});
        }
    }
    ctx.init_text_file_path_n_chk()?;
    let retval = loop {
        cout_n_flush!(">>> ");
        ctx.def.iline = match ctx.stdin_w.lines.next() {
            None => {
                coutln!("Input ended.");
                break Ok(());
            }
            Some(Err(err)) => {
                let l_err: std::io::Error = err;
                break Err(l_err.into());
            }
            Some(Ok(linestr)) => linestr,
        };
        macro_rules! if_no_file_then_noop {
            () => {
                if ctx.def.fsmd.is_none() {
                    coutln!("No file opened.");
                    continue;
                }
            };
        }
        match ctx.def.iline.as_str() {
            "exit" | "quit" => {
                break Ok(());
            }
            "ol" | "oldfiles" => {
                write_bookmark(&mut ctx)?;
                if !oldfiles(&mut ctx)? {
                    break Ok(());
                }
            }
            "g" => {
                //similar to vim ctrl+g
                if_no_file_then_noop!();
                cmd_g(&ctx);
            }
            "rev" => {
                eq_exclam!(ctx.def.reversed);
                println!("{}{}", "rev == ", ctx.def.reversed);
            }
            "+" | " " => {
                if_no_file_then_noop!();
                ctx.def.bookmark_end = ctx.def.bookmark;
                show_prev_page(&mut ctx)?;
            }
            "" => {
                if_no_file_then_noop!();
                if ctx.def.reversed {
                    ctx.def.bookmark_end = ctx.def.bookmark;
                    show_prev_page(&mut ctx)?;
                } else {
                    ctx.def.bookmark = ctx.def.bookmark_end;
                    show_page(&mut ctx)?;
                }
            }
            _ => {
                if ctx.def.iline.starts_with("/") {
                    if_no_file_then_noop!();
                    search_bytes(&mut ctx)?;
                } else if ctx.def.iline.ends_with("%") {
                    if_no_file_then_noop!();
                    percentage_wise(&mut ctx)?;
                } else {
                    coutln!("Command not recognized.");
                }
            }
        }
    };
    write_bookmark(&mut ctx)?;
    retval
}

fn write_bookmark(con: &mut Ctx) -> CustRes<()> {
    if con.def.fsmd.is_some() {
        //note in the future you might add the feature to delete certain HISTORICAL RECORD, so it is important to make DELETE/INSERT/UPDATE of HISTORICAL RECORD atomic. Just before the program exits, HISTORICAL RECORD of current file might have been deleted by another instance, thus when you write bookmark you also need to make sure DB record exists.
        let mut ok: bool = false;
        con.update_open_time_to_now(&mut ok)?;
        if !ok {
            return Err(CustomErr {});
        }
        //fixme `update_open_time_to_now` acquires lock but release it before fs::write. You should release after fs::write. Such that `write_bookmark` becomes really atomic
        fs::write(
            &con.def.text_file_bookmark_path,
            con.def.bookmark.to_string(),
        )?;
    }
    Ok(())
}

fn big_enough_buf_size(con: &Ctx) -> usize {
    //note when do a PgUp the buffer boundary might not fall on code point boundary, so give it a bit more size
    return 256
        + cmp::max(
            2 * con.def_dheight * (con.def_dlwidth - 1 + 2), //note -1 for first column (indicating newline) +2 for CRLF
            con.def_wind_size * 2,
        );
}

trait TextRdr {
    fn clone(&self) -> Box<dyn TextRdr>;
    fn chk_bom(&self, con: &mut Ctx) -> CustRes<u64>;
    fn render(&self, buf: &[u8], rlen: usize, con: &Ctx) -> usize;
    //fn render_prev(&self, buf: &[u8], rlen: usize, con: &Ctx, at_edge: bool) -> usize;
    //fn render_p(&self, con: &Ctx, strs: Vec<(usize, String)>, at_edge: bool) -> usize;

    //fn buf2str<'a>(&self, buf: &'a [u8], rlen: usize) -> borrow::Cow<'a, str>;
    fn buf2strs(&self, buf: &[u8], rlen: usize, at_edge: bool) -> Vec<(usize, String)>; //note return chars and their offset
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct UTF8Rdr;

#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct GB18030Rdr;

impl TextRdr for UTF8Rdr {
    fn clone(&self) -> Box<dyn TextRdr> {
        Box::new(UTF8Rdr {})
    }
    fn chk_bom(&self, con: &mut Ctx) -> CustRes<u64> {
        use std::io::*;
        let fil = con.def.fsfile.as_mut().unwrap();
        fil.seek(io::SeekFrom::Start(0))?;
        const BOM_LEN: u64 = 3;
        let mut buf = vec![0; BOM_LEN as usize];
        match fil.read_exact(&mut buf) {
            Ok(_) => {
                if buf == b"\xEF\xBB\xBF" {
                    //con.def.bookmark = BOM_LEN;
                    return Ok(BOM_LEN);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(err) => {
                return Err(err.into());
            }
        }
        //fil.seek(io::SeekFrom::Start(0))?;
        Ok(0)
    }
    fn render(&self, buf: &[u8], rlen: usize, con: &Ctx) -> usize {
        let cont = &buf[0..rlen];
        let mut retval = 0;
        let mut off = 0;
        let mut lin: String = " ".to_owned();
        let mut height = 0;
        let mut lin_width = 1;
        macro_rules! reset_lin {
            ($new_chr: expr) => {
                lin = $new_chr.to_owned();
                lin_width = 1;
            };
        }
        macro_rules! cout_set_retval {
            () => {
                coutln!(lin);
                retval = off;
            };
        }
        macro_rules! cout_reset {
            () => {
                cout_set_retval!();
                reset_lin!(" ");
                break;
            };
        }
        macro_rules! if_full_then_cout_reset {
            () => {
                if lin_width == con.def_dlwidth {
                    cout_reset!();
                }
            };
        }
        //note do not forget the possibility of trailing invalid utf8 bytes or trailing broken stuff
        let mut depleted = false;
        'height_loop: loop {
            macro_rules! cout_if_needed_break_height {
                () => {
                    if " " != lin {
                        cout_set_retval!();
                    }
                    depleted = true;
                    break 'height_loop;
                };
            }
            loop {
                let byt: u8 = match cont.get(off) {
                    None => {
                        cout_if_needed_break_height!();
                    }
                    Some(inner) => *inner,
                };
                match byt {
                    b'\r' => {
                        off += 1;
                    }
                    b'\n' => {
                        if " " != lin {
                            cout_set_retval!();
                            reset_lin!("$");
                            off += 1;
                            break;
                        }
                        reset_lin!("$");
                        off += 1;
                    }
                    0..=127 => {
                        off += 1;
                        lin.push(byt as char);
                        lin_width += 1;
                        if_full_then_cout_reset!();
                    }
                    //110xxxxx for 2-byte code point
                    0b11000000..=0b11011111 => {
                        let byte1: u8 = match cont.get(off + 1) {
                            None => {
                                cout_if_needed_break_height!();
                            }
                            Some(inner) => *inner,
                        };
                        off += 2;
                        lin.push_str(&String::from_utf8_lossy(&[byt, byte1]));
                        lin_width += 1;
                        if_full_then_cout_reset!();
                    }
                    //1110xxxx for 3-byte code point
                    0b11100000..=0b11101111 => {
                        if lin_width + 2 > con.def_dlwidth {
                            cout_reset!();
                        }
                        let blob = match cont.get(off..off + 3) {
                            None => {
                                cout_if_needed_break_height!();
                            }
                            Some(inner) => inner,
                        };
                        off += 3;
                        lin.push_str(&String::from_utf8_lossy(blob));
                        lin_width += 2;
                        if_full_then_cout_reset!();
                    }
                    //11110xxx for 4-byte code point
                    0b11110000..=0b11110111 => {
                        if lin_width + 2 > con.def_dlwidth {
                            cout_reset!();
                        }
                        let blob = match cont.get(off..off + 4) {
                            None => {
                                cout_if_needed_break_height!();
                            }
                            Some(inner) => inner,
                        };
                        off += 4;
                        lin.push_str(&String::from_utf8_lossy(blob));
                        lin_width += 2;
                        if_full_then_cout_reset!();
                    }
                    _ => {
                        //info!("{}", "BROKEN ENCODED TEXT DETECT");
                        off += 1;
                    }
                }
            }
            height += 1;
            if height == con.def_dheight {
                break;
            }
        }
        if rlen < buf.len() && depleted {
            //? maybe hold back this println if height already full?
            coutln!("END-OF-FILE");
        }
        retval
    }
    fn buf2strs(&self, buf: &[u8], rlen: usize, _at_edge: bool) -> Vec<(usize, String)> {
        let mut retval = vec![];
        let cont = &buf[0..rlen];
        let mut off = 0;
        loop {
            let byt: u8 = match cont.get(off) {
                None => {
                    break;
                }
                Some(inner) => *inner,
            };
            match byt {
                b'\r' => {
                    off += 1;
                }
                0..=127 => {
                    let chr = byt as char;
                    retval.push((off, chr.into()));
                    off += 1;
                }
                //110xxxxx for 2-byte code point
                0b11000000..=0b11011111 => {
                    let byte1: u8 = match cont.get(off + 1) {
                        None => {
                            break;
                        }
                        Some(inner) => *inner,
                    };
                    let lstr = String::from_utf8_lossy(&[byt, byte1]).into_owned();
                    retval.push((off, lstr));
                    off += 2;
                }
                //1110xxxx for 3-byte code point
                0b11100000..=0b11101111 => {
                    let blob = match cont.get(off..off + 3) {
                        None => {
                            break;
                        }
                        Some(inner) => inner,
                    };
                    let lstr = String::from_utf8_lossy(blob).into_owned();
                    retval.push((off, lstr));
                    off += 3;
                }
                //11110xxx for 4-byte code point
                0b11110000..=0b11110111 => {
                    let blob = match cont.get(off..off + 4) {
                        None => {
                            break;
                        }
                        Some(inner) => inner,
                    };
                    let lstr = String::from_utf8_lossy(blob).into_owned();
                    retval.push((off, lstr));
                    off += 4;
                }
                _ => {
                    //info!("{}", "BROKEN ENCODED TEXT DETECT");
                    off += 1;
                }
            }
        }
        retval
    }
    /*
    fn buf2str<'a>(&self, buf: &'a [u8], rlen: usize) -> borrow::Cow<'a, str> {
    //fixme this is not utf8
        String::from_utf8_lossy(&buf[0..rlen])
    }*/
}
impl TextRdr for GB18030Rdr {
    fn clone(&self) -> Box<dyn TextRdr> {
        Box::new(GB18030Rdr {})
    }
    fn chk_bom(&self, con: &mut Ctx) -> CustRes<u64> {
        use std::io::*;
        let fil = con.def.fsfile.as_mut().unwrap();
        fil.seek(io::SeekFrom::Start(0))?;
        const BOM_LEN: u64 = 4;
        let mut buf = vec![0; BOM_LEN as usize];
        match fil.read_exact(&mut buf) {
            Ok(_) => {
                if buf == b"\x84\x31\x95\x33" {
                    //con.def.bookmark = BOM_LEN;
                    return Ok(BOM_LEN);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(err) => {
                return Err(err.into());
            }
        }
        //fil.seek(io::SeekFrom::Start(0))?;
        Ok(0)
    }
    //fn render_prev(&self, buf: &[u8], rlen: usize, con: &Ctx, at_edge: bool) -> usize {
    //    0
    //}
    fn render(&self, buf: &[u8], rlen: usize, con: &Ctx) -> usize {
        let cont = &buf[0..rlen];
        let mut retval = 0;
        let mut off = 0;
        let mut lin: String = " ".to_owned();
        let mut height = 0;
        let mut lin_width = 1;
        macro_rules! reset_lin {
            ($new_chr: expr) => {
                lin = $new_chr.to_owned();
                lin_width = 1;
            };
        }
        macro_rules! cout_set_retval {
            () => {
                coutln!(lin);
                retval = off;
            };
        }
        macro_rules! cout_reset {
            () => {
                cout_set_retval!();
                reset_lin!(" ");
                break;
            };
        }
        macro_rules! if_full_then_cout_reset {
            () => {
                if lin_width == con.def_dlwidth {
                    cout_reset!();
                }
            };
        }
        //note do not forget the possibility of trailing invalid utf8 bytes or trailing broken stuff
        let mut depleted = false;
        'height_loop: loop {
            macro_rules! cout_if_needed_break_height {
                () => {
                    if " " != lin {
                        cout_set_retval!();
                    }
                    depleted = true;
                    break 'height_loop;
                };
            }
            loop {
                let byt: u8 = match cont.get(off) {
                    None => {
                        cout_if_needed_break_height!();
                    }
                    Some(inner) => *inner,
                };
                match byt {
                    b'\r' => {
                        off += 1;
                    }
                    b'\n' => {
                        if " " != lin {
                            cout_set_retval!();
                            reset_lin!("$");
                            off += 1;
                            break;
                        }
                        reset_lin!("$");
                        off += 1;
                    }
                    0..=127 => {
                        off += 1;
                        lin.push(byt as char);
                        lin_width += 1;
                        if_full_then_cout_reset!();
                    }
                    //invalid
                    0x80 | 0xFF => {
                        off += 1;
                    }
                    0x81..=0xFE => {
                        if lin_width + 2 > con.def_dlwidth {
                            cout_reset!();
                        }
                        let byte1: u8 = match cont.get(off + 1) {
                            None => {
                                cout_if_needed_break_height!();
                            }
                            Some(inner) => *inner,
                        };
                        match byte1 {
                            //invalid
                            0x7F | 0xFF => {
                                off += 2;
                                continue;
                            }
                            0x40..=0xFE => {
                                off += 2;
                                use encoding_rs::*;
                                let blob = [byt, byte1];
                                let (cow, _encoding_used, _had_errors) = GB18030.decode(&blob);
                                lin.push_str(&cow);
                                lin_width += 2;
                                if_full_then_cout_reset!();
                                continue;
                            }
                            _ => {}
                        }
                        let blob = match cont.get(off..off + 4) {
                            None => {
                                cout_if_needed_break_height!();
                            }
                            Some(inner) => inner,
                        };
                        off += 4;
                        use encoding_rs::*;
                        let (cow, _encoding_used, _had_errors) = GB18030.decode(blob);
                        lin.push_str(&cow);
                        lin_width += 2;
                        if_full_then_cout_reset!();
                    }
                }
            }
            height += 1;
            if height == con.def_dheight {
                break;
            }
        }
        if rlen < buf.len() && depleted {
            //? maybe hold back this println if height already full?
            coutln!("END-OF-FILE");
        }
        retval
    }
    fn buf2strs(&self, buf: &[u8], rlen: usize, at_edge: bool) -> Vec<(usize, String)> {
        use encoding_rs::*;
        let mut retval: Vec<(usize, String)> = vec![];
        let cont = &buf[0..rlen];
        let mut off = (|| {
            let mut best_idx = 0;
            let mut least_rc = cont.len();
            for idx in 0..4 {
                let (cowstr_def, had_errors) = GB18030.decode_without_bom_handling(&cont[idx..]);
                if !had_errors {
                    return idx;
                }
                let err_count = cowstr_def.matches('\u{FFFD}').count();
                if err_count < least_rc {
                    least_rc = err_count;
                    best_idx = idx;
                }
            }
            return best_idx;
        })();
        loop {
            let byt: u8 = match cont.get(off) {
                None => {
                    break;
                }
                Some(inner) => *inner,
            };
            match byt {
                b'\r' => {
                    off += 1;
                }
                0..=127 => {
                    let chr = byt as char;
                    retval.push((off, chr.into()));
                    off += 1;
                }
                //invalid
                0x80 | 0xFF => {
                    retval.push((off, "\u{FFFD}".into()));
                    off += 1;
                }
                0x81..=0xFE => {
                    let byte1: u8 = match cont.get(off + 1) {
                        None => {
                            break;
                        }
                        Some(inner) => *inner,
                    };
                    match byte1 {
                        //invalid
                        0x7F | 0xFF => {
                            retval.push((off, "\u{FFFD}\u{FFFD}".into()));
                            off += 2;
                            continue;
                        }
                        0x40..=0xFE => {
                            let blob = [byt, byte1];
                            let (cow, _encoding_used, _had_errors) = GB18030.decode(&blob);
                            retval.push((off, cow.into()));
                            off += 2;
                            continue;
                        }
                        _ => {}
                    }
                    let blob = match cont.get(off..off + 4) {
                        None => {
                            break;
                        }
                        Some(inner) => inner,
                    };
                    let (cow, _encoding_used, _had_errors) = GB18030.decode(blob);
                    retval.push((off, cow.into()));
                    off += 4;
                }
            }
        }
        retval
    }
    /*
    fn buf2str<'a>(&self, buf: &'a [u8], rlen: usize) -> borrow::Cow<'a, str> {
    //fixme this is not utf8
        String::from_utf8_lossy(&buf[0..rlen])
    }*/
}

fn show_prev_page(con: &mut Ctx) -> CustRes<()> {
    use std::io::*;
    let mut at_edge = false;
    let bufsize = big_enough_buf_size(con);
    let tr = con.tr.clone();
    let mut bm = if bufsize as u64 > con.def.bookmark_end {
        at_edge = true;
        0
    } else {
        con.def.bookmark_end - bufsize as u64
    };
    if bm < con.def.bom_end {
        at_edge = true;
        bm = con.def.bom_end;
    }
    if bm >= con.def.bookmark_end {
        info!("{}", "Top of file reached.");
        return Ok(());
    }
    let fil = con.def.fsfile.as_mut().unwrap();
    fil.seek(io::SeekFrom::Start(bm))?;
    //let mut buf = vec![0; bufsize];
    let mut buf = vec![0; (con.def.bookmark_end - bm) as usize];
    let rlen = read_to_buf(fil, &mut buf)?;
    let mut strs = tr.buf2strs(&buf, rlen, at_edge);
    strs.retain(|tup| !tup.1.is_empty());
    if strs.is_empty() {
        //note this is reachable when e.g. you have crazy amount of consecutive \r (all characters ignored)
        con.def.bookmark = bm;
        info!("{}", "All characters in this page are non-printable");
        return Ok(());
    }
    con.def.bookmark = bm + render_p(con, strs, at_edge) as u64;
    Ok(())
}

fn render_p(con: &Ctx, mut strs: Vec<(usize, String)>, _at_edge: bool) -> usize {
    let mut retval = 0;
    let mut lin: String = "".to_owned();
    let mut height = 1;
    let mut lin_width = 1;
    loop {
        let (off, cstr) = match strs.pop() {
            None => {
                break;
            }
            Some(inner) => inner,
        };
        macro_rules! mk_newline {
            () => {
                lin.insert(0, '\n');
                height += 1;
                lin_width = 1;
            };
        }
        macro_rules! endline_set_retval {
            () => {
                lin.insert(0, ' ');
                if height == con.def_dheight {
                    break;
                }
                mk_newline!();
            };
        }
        macro_rules! endrealline_set_retval {
            () => {
                lin.insert(0, '$');
                retval = off;
                if height == con.def_dheight {
                    break;
                }
                mk_newline!();
                continue;
            };
        }
        let chr_width = match cstr.len() {
            0 => {
                panic!("This should never be reachable");
            }
            1 => {
                if cstr == "\n" {
                    endrealline_set_retval!();
                }
                1
            }
            2 => 1,
            _ => 2,
        };
        if lin_width + chr_width > con.def_dlwidth {
            endline_set_retval!();
        }
        lin_width += chr_width;
        lin.insert_str(0, &cstr);
        retval = off;
    }
    coutln!(lin);
    retval
}

fn show_page(con: &mut Ctx) -> CustRes<()> {
    //optimize no need to seek every time, only seek when necessary. (Previous leftover can be used for next read)
    //optimize the logic of checking whether enough bytes are read can be incremental instead of re-calculating every time
    use std::io::*;
    //use io::Seek;
    let bufsize = big_enough_buf_size(con);
    let tr = con.tr.clone();
    if con.def.bookmark < con.def.bom_end {
        con.def.bookmark = con.def.bom_end;
    }
    let fil = con.def.fsfile.as_mut().unwrap();
    fil.seek(io::SeekFrom::Start(con.def.bookmark))?;
    let mut buf = vec![0; bufsize];
    let rlen = read_to_buf(fil, &mut buf)?;
    let used_len = tr.render(&buf, rlen, con);
    con.def.bookmark_end = con.def.bookmark + used_len as u64;
    Ok(())
}

fn percentage_wise(con: &mut Ctx) -> CustRes<()> {
    let digits = subsli_cut_rear!(con.def.iline, 1);
    let perc = match digits.parse::<f64>() {
        Err(_) => {
            coutln!("Percentage invalid input.");
            return Ok(());
        }
        Ok(inner) => inner / 100.0,
    };
    let chk_floating_point = perc >= 0f64 && perc <= 1.0; //note this handles NaN etc.
    if !chk_floating_point {
        coutln!("Percentage invalid number.");
        return Ok(());
    }
    let file_len = con.def.fsmd.as_ref().unwrap().len();
    //"Casting from an integer to float will produce the closest possible float"
    //"Casting from a float to an integer will round the float towards zero"
    let mut bm = (file_len as f64 * perc) as u64;
    if bm > file_len {
        bm = file_len;
    }
    con.def.bookmark = bm;
    con.def.bookmark_end = bm;
    show_page(con)?;
    Ok(())
}

fn cmd_g(con: &Ctx) {
    coutln!(con.def.text_file_path_str);
    let file_len = con.def.fsmd.as_ref().unwrap().len();
    let perc = con.def.bookmark * 100 / file_len;
    println!("{}{}", perc, "%");
    println!("{}{}{}", con.def.bookmark, "/", file_len);
}

fn search_bytes(con: &mut Ctx) -> CustRes<()> {
    //fixme this method is safe for UTF8 but not safe for some other encoding schemes (search raw bytes might end up in the middle of a multi-byte code point)
    use std::io::*;
    debug_assert!(con.def.iline.len() != 0);
    if con.def.iline.len() == 1 {
        info!("{}", "Cannot search without input");
        return Ok(());
    }
    let (blob, _enc, res) = con.enc.encode(&con.def.iline[1..]);
    if res {
        info!("{}", "Unmappable characters in input");
        return Ok(());
    }
    let fil = con.def.fsfile.as_mut().unwrap();
    fil.seek(io::SeekFrom::Start(con.def.bookmark))?;
    let mut buf = vec![0; cmp::max(blob.len() * 0x100, 0x10000)];
    let idx = match read_n_find(fil, &mut buf, &blob)? {
        None => {
            info!("{}", "Not found.");
            return Ok(());
        }
        Some(inner) => inner as u64,
    };
    con.def.bookmark += idx;
    con.def.bookmark_end = con.def.bookmark;
    show_page(con)?;
    Ok(())
}

fn open_text(con: &mut Ctx, filenm: &str) -> CustRes<()> {
    hash_fpath!(con, filenm);
    con.text_file_bookmark_path = con.bookmark_dir.join(&con.def.text_file_path_hash);
    if con.text_file_bookmark_path.try_exists()? {
        if !real_reg_file_without_symlink(&con.def.text_file_bookmark_path) {
            return Err("Caret file is not regular file".into());
        }
        con.bookmark = fs::read_to_string(&con.def.text_file_bookmark_path)?.parse()?;
        info!("{}", "BOOKMARK found.");
    }
    let mut ok: bool = false;
    con.update_open_time_to_now(&mut ok)?;
    if !ok {
        return Err(CustomErr {});
    }
    let fil = fs::File::open(&con.def.text_file_path)?;
    let md = fil.metadata()?;
    //note you must allow md.len() == con.def.bookmark, because when you open empty file this happens naturally
    if md.len() < con.def.bookmark {
        con.def.bookmark = 0; //todo better handling?
    }
    con.def.fsmd = Some(md);
    con.def.fsfile = Some(fil);
    let tr = con.tr.clone();
    con.def.bom_end = tr.chk_bom(con)?;
    show_page(con)?;
    Ok(())
}

fn oldfiles(con: &mut Ctx) -> CustRes<bool> {
    let mut ok: bool = false;
    let fullp = oldfiles_lst(con, &mut ok)?;
    if !ok {
        return Err(CustomErr {});
    }
    let mut idx = cmp::min(DEF_OLDFILES_LST_LEN, fullp.len());
    while idx != 0 {
        idx -= 1;
        println!("{} {}", idx, fullp[idx])
    }
    let fnmstr = loop {
        cout_n_flush!("Files listed as above. Choose one or input `full` for full list (Or empty input to cancel): ");
        let choice = match con.def.stdin_w.lines.next() {
            None => {
                warn!("{}", "Unexpected stdin EOF");
                return Ok(false);
            }
            Some(Err(err)) => {
                let l_err: std::io::Error = err;
                return Err(l_err.into());
            }
            Some(Ok(linestr)) => linestr,
        };
        match choice.as_str() {
            "" => {
                return Ok(true);
            }
            "full" => {
                for (idx, filep) in fullp.iter().enumerate() {
                    println!("{} {}", idx, filep)
                }
                continue;
            }
            _ => {}
        }
        let chosen = choice.parse::<usize>();
        let chosen_idx = match chosen {
            Err(_) => {
                coutln!("Invalid index");
                continue;
            }
            Ok(l_idx) => l_idx,
        };
        let chosen_f = fullp.get(chosen_idx);
        break match chosen_f {
            None => {
                coutln!("Invalid index");
                continue;
            }
            Some(valid_f) => valid_f,
        };
    };
    open_text(con, fnmstr)?;
    Ok(true)
}
fn oldfiles_lst(con: &Ctx, ok: &mut bool) -> CustRes<Vec<String>> {
    let fobj = monitor_enter(&con.def.lock_p)?;
    defer! {
        *ok = monitor_exit(fobj); //?ignore retval?
    }
    oldfiles_lst_inner(con)
}
fn oldfiles_lst_inner(con: &Ctx) -> CustRes<Vec<String>> {
    let db = con.open_db()?;
    let fullp = {
        let mut st = db.prepare("select fullpath from files order by open_time desc")?;
        query_n_collect_into_vec_string(st.query([]))?
    };
    Ok(fullp)
}

struct Ctx {
    //db: rusqlite::Connection,
    hasher: sha2::Sha256,
    args: Vec<String>,
    tr: Box<dyn TextRdr>,
    enc: &'static encoding_rs::Encoding,
    def: CtxDef,
}
impl ops::Deref for Ctx {
    type Target = CtxDef;

    fn deref(&self) -> &CtxDef {
        &self.def
    }
}
impl ops::DerefMut for Ctx {
    fn deref_mut(&mut self) -> &mut CtxDef {
        &mut self.def
    }
}

#[derive(Default)]
struct CtxDef {
    stdin_w: StdinWrapper,
    def_dlwidth: usize,
    def_dheight: usize,
    def_enc_scheme: String,
    def_wind_size: usize,
    def_cache_size: usize,
    home_dir: PathBuf,
    everycom: PathBuf,
    app_support_dir: PathBuf,
    bookmark_dir: PathBuf,
    db_p: PathBuf,
    lock_p: PathBuf,
    text_file_path: PathBuf,
    text_file_path_str: String,
    text_file_path_hash: String,
    text_file_bookmark_path: PathBuf,
    bookmark: u64,
    bookmark_end: u64,
    bom_end: u64,
    iline: String,
    reversed: bool,
    //show_line_number: bool,//todo
    fsfile: Option<fs::File>,
    fsmd: Option<fs::Metadata>,
}
struct StdinWrapper {
    lines: std::io::Lines<std::io::StdinLock<'static>>,
}
impl Default for StdinWrapper {
    fn default() -> Self {
        info!("{}", "READING STDIN");
        let stdin = io::stdin();
        use std::io::prelude::*;
        Self {
            lines: stdin.lock().lines(),
        }
    }
}

fn del_records_where(con: &mut Ctx, ok: &mut bool, del_where: String) -> CustRes<()> {
    let fobj = monitor_enter(&con.def.lock_p)?;
    defer! {
        *ok = monitor_exit(fobj); //?ignore retval?
    }
    del_records_where_inner(con, del_where)
}

fn del_records_of_nonexistent_files(con: &mut Ctx, ok: &mut bool) -> CustRes<()> {
    let fobj = monitor_enter(&con.def.lock_p)?;
    defer! {
        *ok = monitor_exit(fobj); //?ignore retval?
    }
    del_records_of_nonexistent_files_inner(con)
}

const SEL_SQL: &str = "select fullpath from files where ";

fn del_records_where_inner(con: &mut Ctx, mut del_where: String) -> CustRes<()> {
    coutln!("CHECKING HISTORY RECORDS FOR DELETION BEGIN");
    let mut db = con.open_db()?;
    let tx = db.transaction()?; //when this var drops, it calls roolback by default
    del_where.insert_str(0, SEL_SQL);
    let fullp = {
        let mut st = tx.prepare(&del_where)?;
        query_n_collect_into_vec_string(st.query([]))?
    };
    {
        let mut st = tx.prepare("delete from files where fullpath=?1")?;
        for fullpath in fullp {
            println!("{}{}", "DEL HIST REC ", fullpath);
            let bm = sha256hex_of_str(&mut con.hasher, &fullpath)?;
            let bmpath = con.bookmark_dir.join(bm);
            st.execute((fullpath,))?;
            if let Err(err) = fs::remove_file(bmpath) {
                error!("{}{}", "ERR during remove_file: ", err);
            }
        }
    }
    tx.commit()?;
    coutln!("CHECKING HISTORY RECORDS FOR DELETION END");
    Ok(())
}
fn del_records_of_nonexistent_files_inner(con: &mut Ctx) -> CustRes<()> {
    coutln!("CHECKING FILE EXISTENCE BEGIN");
    let mut db = con.open_db()?;
    let tx = db.transaction()?; //when this var drops, it calls roolback by default
    let fullp = {
        let mut st = tx.prepare("select fullpath from files")?;
        query_n_collect_into_vec_string(st.query([]))?
    };
    {
        let mut st = tx.prepare("delete from files where fullpath=?1")?;
        for fullpath in fullp {
            if real_reg_file_without_symlink(path::Path::new(&fullpath)) {
                continue;
            }
            println!("{}{}", "DEL HIST REC ", fullpath);
            let bm = sha256hex_of_str(&mut con.hasher, &fullpath)?;
            let bmpath = con.bookmark_dir.join(bm);
            st.execute((fullpath,))?;
            if let Err(err) = fs::remove_file(bmpath) {
                error!("{}{}", "ERR during remove_file: ", err);
            }
        }
    }
    tx.commit()?;
    coutln!("CHECKING FILE EXISTENCE END");
    Ok(())
}

impl Ctx {
    //fn list_recent_files(&mut self) -> CustRes<()> {
    //    if self.recent_files_p.try_exists()? {
    //        if !util::real_reg_file_without_symlink(&self.recent_files_p) {
    //            return Err("Recent files listing file is not regular file".into());
    //        }
    //        self.recent_files = fs::read_to_string(&self.recent_files_p)?
    //            .split('\n')
    //            .map(|vstr| vstr.to_owned())
    //            .collect();
    //        for (idx, rfile) in self.recent_files.iter().enumerate() {
    //            println!("{} {}", idx, rfile);
    //        }
    //    }
    //    Ok(())
    //}
    fn init_text_file_path_n_chk(&mut self) -> Result<(), CustomErr> {
        let filenm: &str = match self.args.get(1) {
            None => {
                coutln!("No file opened.");
                return Ok(());
            }
            Some(vstr) => vstr,
        };
        open_text(self, &filenm.to_owned())
    }
    fn update_open_time_to_now_inner(&self) -> CustRes<()> {
        let mut db = self.open_db()?;
        let tx = db.transaction()?; //when this var drops, it calls roolback by default
        let res_rows_empty = {
            let mut st = tx.prepare("select 0 from files where fullpath=?1")?;
            result_rows_empty(st.query((&self.def.text_file_path_str,)))?
        };
        if res_rows_empty {
            tx.execute(
                "insert into files values(?1,?2)",
                (&self.def.text_file_path_str, now_in_millis()),
            )?;
        } else {
            tx.execute(
                "update files set open_time=?1 where fullpath=?2",
                (now_in_millis(), &self.def.text_file_path_str),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    fn update_open_time_to_now(&self, ok: &mut bool) -> CustRes<()> {
        let fobj = monitor_enter(&self.def.lock_p)?;
        defer! {
            *ok = monitor_exit(fobj); //?ignore retval?
        }
        self.update_open_time_to_now_inner()
    }
    fn open_db(&self) -> CustRes<rusqlite::Connection> {
        use rusqlite::Connection;
        let existing = self.db_p.try_exists()?;
        let db = Connection::open(&self.db_p)?;
        if !existing {
            init_sqlite(&db)?;
        }
        Ok(db)
    }
}

fn monitor_enter(lock_p: &path::Path) -> CustRes<fs::File> {
    file_lock(lock_p, b"\n")
}
fn monitor_exit(fobj: fs::File) -> bool {
    file_unlock(fobj)
}

fn init_sqlite(conn: &rusqlite::Connection) -> Result<(), CustomErr> {
    conn.execute(
        "CREATE TABLE files (
	    fullpath text not null,
	    open_time integer not null
        )",
        (),
    )?;
    conn.execute("CREATE INDEX idx_open_time ON files (open_time)", ())?;
    conn.execute("CREATE INDEX idx_fullpath ON files (fullpath)", ())?;
    Ok(())
}
