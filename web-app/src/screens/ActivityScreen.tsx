import { useRef, useState, useCallback } from "react";
import { useTheme } from "../ThemeContext";
import { useAppNavigation } from "../context/NavigationContext";
import { TransactionItem } from "../components/TransactionItem";
import { type Transaction } from "../data/mockData";
import { fonts } from "../theme";
import { Ionicons } from "../components/Ionicons";
import { useVirtualizer } from "@tanstack/react-virtual";

export const ActivityScreen = ({ txs }: { txs: Transaction[] }) => {
  const { c } = useTheme();
  const { navigate, goBack } = useAppNavigation();
  const [displayedTxs, setDisplayedTxs] = useState<Transaction[]>(txs.slice(0, 50)); // Initial batch of 50
  const [isLoading, setIsLoading] = useState(false);
  
  const parentRef = useRef<HTMLDivElement>(null);

  // Virtual row configuration
  const rowVirtualizer = useVirtualizer({
    count: displayedTxs.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 80, // Estimated height of each transaction item
    overscan: 5, // Render 5 extra items above/below viewport for smooth scrolling
  });

  // Infinite scroll handler
  const handleScroll = useCallback(() => {
    if (!parentRef.current || isLoading) return;

    const { scrollTop, scrollHeight, clientHeight } = parentRef.current;
    const scrollPercentage = (scrollTop + clientHeight) / scrollHeight;

    // Load more when user scrolls to 80% of the list
    if (scrollPercentage > 0.8 && displayedTxs.length < txs.length) {
      setIsLoading(true);
      // Simulate API delay and load next batch
      setTimeout(() => {
        const nextBatchSize = 50;
        const currentLength = displayedTxs.length;
        const nextBatch = txs.slice(currentLength, currentLength + nextBatchSize);
        setDisplayedTxs(prev => [...prev, ...nextBatch]);
        setIsLoading(false);
      }, 300);
    }
  }, [displayedTxs.length, txs.length, isLoading]);

  const virtualItems = rowVirtualizer.getVirtualItems();

  return (
    <div className="fade-in">
      {/* Header */}
      <div style={{ display: "flex", alignItems: "center", gap: "12px", marginBottom: "32px" }}>
        <button onClick={goBack} style={{ padding: "8px", borderRadius: "12px", display: "flex", alignItems: "center", justifyContent: "center" }}>
          <Ionicons name="chevron-back" size={24} color={c.foreground} />
        </button>
        <span style={{ fontSize: "22px", fontWeight: "800", color: c.foreground, fontFamily: fonts.display, letterSpacing: "-0.5px" }}>Activity Ledger</span>
      </div>

      <div 
        className="glass-card" 
        style={{ 
          borderRadius: "28px", 
          padding: "12px",
          height: "600px",
          overflow: "auto",
          position: "relative"
        }}
        ref={parentRef}
        onScroll={handleScroll}
      >
        {/* Spacer at top for virtual scrolling */}
        <div
          style={{
            height: `${virtualItems.length > 0 ? virtualItems[0]?.start || 0 : 0}px`,
          }}
        />

        {/* Virtual items */}
        {virtualItems.map((virtualItem) => {
          const tx = displayedTxs[virtualItem.index];
          if (!tx) return null;

          return (
            <div
              key={virtualItem.key}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${virtualItem.start}px)`,
              }}
            >
              <TransactionItem
                transaction={tx}
                onClick={() => navigate("TransactionDetail", { id: tx.id })}
              />
            </div>
          );
        })}

        {/* Spacer at bottom for virtual scrolling */}
        <div
          style={{
            height: `${virtualItems.length > 0 ? rowVirtualizer.getTotalSize() - (virtualItems[virtualItems.length - 1]?.end || 0) : 0}px`,
          }}
        />

        {/* Loading indicator */}
        {isLoading && (
          <div style={{ 
            padding: "20px", 
            textAlign: "center", 
            color: c.mutedForeground,
            fontSize: "14px"
          }}>
            Loading more transactions...
          </div>
        )}

        {/* End of list indicator */}
        {!isLoading && displayedTxs.length >= txs.length && (
          <div style={{ 
            padding: "20px", 
            textAlign: "center", 
            color: c.mutedForeground,
            fontSize: "14px"
          }}>
            Showing all {txs.length} transactions
          </div>
        )}
      </div>
    </div>
  );
};
