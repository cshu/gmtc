//pub fn millis2display(ms: i64) -> String {
//    use chrono::prelude::*;
//    let ndt = NaiveDateTime::from_timestamp_millis(ms);
//    let naive = match ndt {
//        None => {
//            return "FAILED TO CONV MS TO STR".to_owned();
//        }
//        Some(ndt_v) => ndt_v,
//    };
//    let datetime: DateTime<Utc> = DateTime::from_utc(naive, Utc);
//    datetime.to_string()
//}
//

#[macro_export]
macro_rules! hash_fpath {
    ($self: ident, $filenm: expr) => {
        //note canonicalize returns err if file does not exist
        $self.def.text_file_path = std::fs::canonicalize($filenm)?;
        $self.def.text_file_path_hash = sha256hex_of_path(
            &mut $self.hasher,
            &mut $self.def.text_file_path_str,
            &$self.def.text_file_path,
        )?;
    };
}

//#[macro_export]
//macro_rules! render_out_byte {
//	($byt: expr) => {
//		print!("{}", $byt as char);
//	};
//}
