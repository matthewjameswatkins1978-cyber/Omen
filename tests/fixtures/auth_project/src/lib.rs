pub fn authenticate(token: &str) -> bool {
    token == "omen-valid-token"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_success() {
        assert!(authenticate("omen-valid-token"));
    }
}
