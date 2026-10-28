// SPDX-License-Identifier: Apache-2.0
pub(super) fn read_secret() -> std::io::Result<String> {
    let term = console::Term::stdout();
    if !term.is_term() {
        return Err(std::io::Error::other("Secret input requires a terminal; use --anthropic-api-key-env for non-interactive setup"));
    }
    let value = read_keys(|| term.read_key_raw())?;
    term.write_line("")?;
    Ok(value)
}

fn read_keys(mut read: impl FnMut() -> std::io::Result<console::Key>) -> std::io::Result<String> {
    let mut value = String::new();
    loop {
        match read()? {
            console::Key::Enter => return Ok(value),
            console::Key::Backspace => {
                value.pop();
            }
            console::Key::Char(ch) if !ch.is_control() => value.push(ch),
            console::Key::CtrlC | console::Key::Escape => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Secret input cancelled",
                ))
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn backspace_corrects_the_secret_on_unicode_boundaries() {
        use console::Key;
        let mut keys = [
            Key::Backspace,
            Key::Char('中'),
            Key::Char('🙂'),
            Key::Backspace,
            Key::Char('x'),
            Key::Enter,
        ]
        .into_iter();
        assert_eq!(
            super::read_keys(|| Ok(keys.next().unwrap())).unwrap(),
            "中x"
        );
        assert!(super::read_keys(|| Ok(Key::CtrlC)).is_err());
    }
    #[test]
    fn redirected_input_cannot_fall_back_to_plaintext() {
        if !console::Term::stdout().is_term() {
            assert!(super::read_secret().is_err());
        }
    }
}
