CREATE TABLE sep24_transactions (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    asset_code TEXT NOT NULL,
    account TEXT NOT NULL,
    amount_in TEXT,
    amount_out TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
