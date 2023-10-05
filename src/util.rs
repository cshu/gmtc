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
