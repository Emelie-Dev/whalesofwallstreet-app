import { memo } from "react";
import { useTheme } from "../ThemeContext";
import { type Transaction } from "../data/mockData";
import { Ionicons } from "./Ionicons";

export const TransactionItem = memo(({ transaction, onClick }: { transaction: Transaction; onClick: () => void }) => {
  const { c } = useTheme();
  const isReceived = transaction.type === "received";

  return (
    <div
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        gap: "16px",
        padding: "14px 18px",
        borderRadius: "20px",
        cursor: "pointer",
        transition: "background 0.2s",
      }}
      className="tx-item-hover"
    >
      <div style={{ position: "relative" }}>
        <img
          src={transaction.avatar}
          alt={transaction.name}
          style={{ width: "48px", height: "48px", borderRadius: "18px", backgroundColor: c.secondary, objectFit: "cover" }}
        />
        <div
          style={{
            position: "absolute",
            bottom: "-4px",
            right: "-4px",
            width: "20px",
            height: "20px",
            borderRadius: "50%",
            backgroundColor: isReceived ? c.success : c.primary,
            border: `2px solid ${c.card}`,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <Ionicons
            name="arrow-forward"
            size={10}
            color="#fff"
            style={{ transform: `rotate(${isReceived ? "135deg" : "-45deg"})` }}
          />
        </div>
      </div>

      <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: "3px" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span style={{ fontSize: "15px", fontWeight: "700", color: c.foreground }}>{transaction.name}</span>
          <span style={{ fontSize: "15px", fontWeight: "800", color: isReceived ? c.success : c.foreground }}>
            {isReceived ? "+" : "−"}${transaction.amount.toFixed(2)}
          </span>
        </div>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span style={{ fontSize: "13px", color: c.mutedForeground }}>
            {transaction.note || (isReceived ? "Received" : "Sent")}
          </span>
          <div style={{ display: "flex", alignItems: "center", gap: "6px" }}>
            {transaction.status === "pending" && (
              <div style={{ width: "6px", height: "6px", borderRadius: "50%", backgroundColor: c.warning }} />
            )}
            <span style={{ fontSize: "12px", color: c.mutedForeground }}>{transaction.date}</span>
          </div>
        </div>
      </div>
    </div>
  );
});
