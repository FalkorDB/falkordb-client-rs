/*
 * Copyright FalkorDB Ltd. 2023 - present
 * Licensed under the Server Side Public License v1 (SSPLv1).
 */

use anyhow::Result;

#[derive(Clone)]
pub enum FalkorConnectionInfo {
    #[cfg(feature = "redis")]
    Redis(redis::ConnectionInfo),
}

impl FalkorConnectionInfo {
    /// Redis is currently only option, and I feel will be the default for a while
    /// So any new option should be set here should be added before redis
    /// as a #[cfg(and(feature = <provider>, not(feature = "redis"))]
    fn fallback_provider(full_url: String) -> Result<FalkorConnectionInfo> {
        #[cfg(feature = "redis")]
        Ok(FalkorConnectionInfo::Redis(
            redis::IntoConnectionInfo::into_connection_info(format!("redis://{full_url}"))?,
        ))
    }
}

impl TryFrom<&str> for FalkorConnectionInfo {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let url = url_parse::core::Parser::new(None).parse(value)?;

        let scheme = url.scheme.unwrap_or("falkor".to_string());
        let addr = url.domain.unwrap_or("127.0.0.1".to_string());
        let port = url.port.unwrap_or(6379); // Might need to change in accordance with the default fallback

        let user_pass_string = match url.user_pass {
            (Some(pass), None) => format!("{}@", pass), // Password-only authentication is allowed in legacy auth
            (Some(user), Some(pass)) => format!("{user}:{pass}@"),
            _ => "".to_string(),
        };

        match scheme.as_str() {
            "redis" | "rediss" => {
                #[cfg(feature = "redis")]
                return Ok(FalkorConnectionInfo::Redis(
                    redis::IntoConnectionInfo::into_connection_info(format!(
                        "{}://{}{}:{}",
                        scheme, user_pass_string, addr, port
                    ))?,
                ));
                #[cfg(not(feature = "redis"))]
                return Err(FalkorDBError::UnavailableProvider.into());
            }
            _ => FalkorConnectionInfo::fallback_provider(format!(
                "{}{}:{}",
                user_pass_string, addr, port
            )),
        }
    }
}

// Calls TryFrom<&str>
impl TryFrom<String> for FalkorConnectionInfo {
    type Error = anyhow::Error;

    #[inline]
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

// Calls TryFrom<String>
impl<T: ToString> TryFrom<(T, u16)> for FalkorConnectionInfo {
    type Error = anyhow::Error;

    #[inline]
    fn try_from(value: (T, u16)) -> Result<Self, Self::Error> {
        Self::try_from(format!("{}:{}", value.0.to_string(), value.1))
    }
}
