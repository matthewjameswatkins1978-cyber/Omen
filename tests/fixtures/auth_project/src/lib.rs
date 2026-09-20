pub struct SessionToken {
    pub value: String,
}

pub fn authenticate(token: &str) -> bool {
    token == "omen-valid-token"
}

pub fn refresh_token(token: &SessionToken) -> String {
    token.value.clone()
}

pub fn use_refresh(token: &SessionToken) -> String {
    refresh_token(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_success() {
        assert!(authenticate("omen-valid-token"));
    }
}
