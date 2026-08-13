use async_trait::async_trait;
use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::bridge::Chain;
use crate::error::AppError;

use super::validation::{validate_asset_code, validate_stellar_address};

#[derive(Deserialize, Debug)]
pub struct ValidatedQuoteRequest {
    pub source_chain: Chain,
    pub dest_chain: Chain,
    pub source_asset: String,
    pub dest_asset: String,
    pub amount_in: u64,
}

impl ValidatedQuoteRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.source_asset.trim().is_empty() {
            return Err(AppError::BadRequest("Source asset cannot be empty".into()));
        }
        if self.dest_asset.trim().is_empty() {
            return Err(AppError::BadRequest(
                "Destination asset cannot be empty".into(),
            ));
        }
        if self.amount_in == 0 {
            return Err(AppError::BadRequest(
                "Amount in must be greater than zero".into(),
            ));
        }
        validate_chain_asset_compat(self.source_chain, &self.source_asset, "source")?;
        validate_chain_asset_compat(self.dest_chain, &self.dest_asset, "destination")?;
        Ok(())
    }
}

#[async_trait]
impl<S> FromRequest<S> for ValidatedQuoteRequest
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(payload) = Json::<Self>::from_request(req, state)
            .await
            .map_err(|e| e.into_response())?;
        payload.validate().map_err(|e| e.into_response())?;
        Ok(payload)
    }
}

#[derive(Deserialize, Debug)]
pub struct ValidatedDepositRequest {
    pub anchor_domain: String,
    pub asset_code: String,
    pub account: String,
}

impl ValidatedDepositRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        validate_domain(&self.anchor_domain)?;
        if let Err(e) = validate_stellar_address(&self.account) {
            return Err(AppError::BadRequest(format!(
                "Invalid account address: {}",
                e
            )));
        }
        if let Err(e) = validate_asset_code(&self.asset_code) {
            return Err(AppError::BadRequest(format!("Invalid asset code: {}", e)));
        }
        Ok(())
    }
}

#[async_trait]
impl<S> FromRequest<S> for ValidatedDepositRequest
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(payload) = Json::<Self>::from_request(req, state)
            .await
            .map_err(|e| e.into_response())?;
        payload.validate().map_err(|e| e.into_response())?;
        Ok(payload)
    }
}

#[derive(Deserialize, Debug)]
pub struct ValidatedWithdrawRequest {
    pub anchor_domain: String,
    pub asset_code: String,
    pub account: String,
}

impl ValidatedWithdrawRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        validate_domain(&self.anchor_domain)?;
        if let Err(e) = validate_stellar_address(&self.account) {
            return Err(AppError::BadRequest(format!(
                "Invalid account address: {}",
                e
            )));
        }
        if let Err(e) = validate_asset_code(&self.asset_code) {
            return Err(AppError::BadRequest(format!("Invalid asset code: {}", e)));
        }
        Ok(())
    }
}

#[async_trait]
impl<S> FromRequest<S> for ValidatedWithdrawRequest
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(payload) = Json::<Self>::from_request(req, state)
            .await
            .map_err(|e| e.into_response())?;
        payload.validate().map_err(|e| e.into_response())?;
        Ok(payload)
    }
}

#[derive(Deserialize, Debug)]
pub struct ValidatedAnchorQuoteRequest {
    pub anchor_domain: String,
    pub sell_asset: String,
    pub buy_asset: String,
    pub sell_amount: f64,
}

impl ValidatedAnchorQuoteRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        validate_domain(&self.anchor_domain)?;
        if let Err(e) = validate_asset_code(&self.sell_asset) {
            return Err(AppError::BadRequest(format!("Invalid sell asset: {}", e)));
        }
        if let Err(e) = validate_asset_code(&self.buy_asset) {
            return Err(AppError::BadRequest(format!("Invalid buy asset: {}", e)));
        }
        if self.sell_amount <= 0.0 {
            return Err(AppError::BadRequest(
                "Sell amount must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl<S> FromRequest<S> for ValidatedAnchorQuoteRequest
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(payload) = Json::<Self>::from_request(req, state)
            .await
            .map_err(|e| e.into_response())?;
        payload.validate().map_err(|e| e.into_response())?;
        Ok(payload)
    }
}

#[derive(Deserialize, Debug)]
pub struct ValidatedExecuteRouteRequest {
    pub user_id: Uuid,
    pub source_chain: String,
    pub dest_chain: String,
    pub source_asset: String,
    pub dest_asset: String,
    pub amount_in: u64,
    pub amount_out: u64,
    pub provider: String,
    pub path: String,
    pub estimated_fee_usd: f64,
    pub anchor_domain: Option<String>,
    pub anchor_transaction_id: Option<String>,
}

impl ValidatedExecuteRouteRequest {
    pub fn validate_and_convert(
        &self,
    ) -> Result<(crate::bridge::Chain, crate::bridge::Chain), AppError> {
        if self.amount_in == 0 {
            return Err(AppError::BadRequest(
                "Amount in must be greater than zero".into(),
            ));
        }
        if self.amount_out == 0 {
            return Err(AppError::BadRequest(
                "Amount out must be greater than zero".into(),
            ));
        }
        if self.estimated_fee_usd < 0.0 {
            return Err(AppError::BadRequest(
                "Estimated fee cannot be negative".into(),
            ));
        }
        if self.source_asset.trim().is_empty() {
            return Err(AppError::BadRequest("Source asset cannot be empty".into()));
        }
        if self.dest_asset.trim().is_empty() {
            return Err(AppError::BadRequest(
                "Destination asset cannot be empty".into(),
            ));
        }
        if self.provider.trim().is_empty() {
            return Err(AppError::BadRequest("Provider cannot be empty".into()));
        }
        if self.path.trim().is_empty() {
            return Err(AppError::BadRequest("Path cannot be empty".into()));
        }

        let source_chain = parse_chain(&self.source_chain)?;
        let dest_chain = parse_chain(&self.dest_chain)?;

        validate_chain_asset_compat(source_chain, &self.source_asset, "source")?;
        validate_chain_asset_compat(dest_chain, &self.dest_asset, "destination")?;

        Ok((source_chain, dest_chain))
    }
}

#[async_trait]
impl<S> FromRequest<S> for ValidatedExecuteRouteRequest
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(payload) = Json::<Self>::from_request(req, state)
            .await
            .map_err(|e| e.into_response())?;
        payload
            .validate_and_convert()
            .map_err(|e| e.into_response())?;
        Ok(payload)
    }
}

fn validate_chain_asset_compat(
    chain: crate::bridge::Chain,
    asset: &str,
    label: &str,
) -> Result<(), AppError> {
    match chain {
        crate::bridge::Chain::Stellar => {
            if let Err(e) = validate_asset_code(asset) {
                return Err(AppError::BadRequest(format!(
                    "Invalid {} asset for Stellar chain: {}",
                    label, e
                )));
            }
        }
        crate::bridge::Chain::Ethereum
        | crate::bridge::Chain::Arbitrum
        | crate::bridge::Chain::Solana => {
            if asset.is_empty() {
                return Err(AppError::BadRequest(format!(
                    "{} asset cannot be empty for {:?}",
                    label, chain
                )));
            }
            if asset.len() > 20 {
                return Err(AppError::BadRequest(format!(
                    "{} asset code too long for {:?} (max 20 chars)",
                    label, chain
                )));
            }
            for c in asset.chars() {
                if !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.' {
                    return Err(AppError::BadRequest(format!(
                        "Invalid character '{}' in {} asset for {:?}",
                        c, label, chain
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_domain(domain: &str) -> Result<(), AppError> {
    if domain.trim().is_empty() {
        return Err(AppError::BadRequest("Anchor domain cannot be empty".into()));
    }
    if domain.contains(' ') {
        return Err(AppError::BadRequest(
            "Anchor domain cannot contain spaces".into(),
        ));
    }
    Ok(())
}

fn parse_chain(s: &str) -> Result<crate::bridge::Chain, AppError> {
    match s {
        "Ethereum" => Ok(crate::bridge::Chain::Ethereum),
        "Solana" => Ok(crate::bridge::Chain::Solana),
        "Arbitrum" => Ok(crate::bridge::Chain::Arbitrum),
        "Stellar" => Ok(crate::bridge::Chain::Stellar),
        _ => Err(AppError::BadRequest(format!(
            "Invalid chain: '{}'. Must be one of: Ethereum, Solana, Arbitrum, Stellar",
            s
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_domain() {
        assert!(validate_domain("example.com").is_ok());
        assert!(validate_domain("").is_err());
    }

    #[test]
    fn test_parse_chain() {
        assert!(parse_chain("Ethereum").is_ok());
        assert!(parse_chain("Bitcoin").is_err());
    }
}
