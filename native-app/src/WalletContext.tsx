import React, { createContext, useContext } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { currentUser as initialUser, transactions as initialTransactions, Transaction } from "./data/mockData";

type WalletPortfolio = {
  balance: number;
  transactions: Transaction[];
};

type WalletContextType = {
  balance: number;
  transactions: Transaction[];
  addTransaction: (tx: Omit<Transaction, "id" | "date" | "status">) => void;
  deposit: (amount: number, method: string) => void;
  withdraw: (amount: number, bank: string) => void;
  isDepositing: boolean;
  isWithdrawing: boolean;
  isOffline: boolean;
  isPortfolioUnavailable: boolean;
  refreshPortfolio: () => Promise<void>;
};

const WalletContext = createContext<WalletContextType | undefined>(undefined);

const API_BASE_URL = process.env.EXPO_PUBLIC_API_BASE_URL || "http://localhost:8080";
const portfolioQueryKey = ["wallet-portfolio"] as const;

const loadWalletPortfolio = async (): Promise<WalletPortfolio> => ({
  balance: initialUser.balance,
  transactions: initialTransactions,
});

export const WalletProvider = ({ children }: { children: React.ReactNode }) => {
  const queryClient = useQueryClient();
  const portfolioQuery = useQuery({
    queryKey: portfolioQueryKey,
    queryFn: loadWalletPortfolio,
    staleTime: Infinity,
  });
  const portfolio = portfolioQuery.data;
  const balance = portfolio?.balance ?? 0;
  const transactions = portfolio?.transactions ?? [];
  const isOffline = portfolioQuery.fetchStatus === "paused";
  const isPortfolioUnavailable = !portfolio && (isOffline || portfolioQuery.isError);

  const updatePortfolio = (updater: (current: WalletPortfolio) => WalletPortfolio) => {
    queryClient.setQueryData<WalletPortfolio>(portfolioQueryKey, (current) =>
      current ? updater(current) : current,
    );
  };

  const addTransaction = (newTx: Omit<Transaction, "id" | "date" | "status">) => {
    const tx: Transaction = {
      ...newTx,
      id: `t_${Date.now()}`,
      date: "Just now",
      status: "completed",
    };
    const amountChange = newTx.type === "sent" ? -newTx.amount : newTx.amount;
    updatePortfolio((current) => ({
      ...current,
      balance: current.balance + amountChange,
      transactions: [tx, ...current.transactions],
    }));
  };

  const depositMutation = useMutation({
    mutationFn: async ({ amount, method }: { amount: number, method: string }) => {
      const res = await fetch(`${API_BASE_URL}/api/v1/anchor/deposit`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          anchor_domain: method,
          asset_code: "USDC",
          account: initialUser.stellarAddress
        })
      });
      if (!res.ok) throw new Error("Deposit failed");
      return { amount, method };
    },
    onSuccess: ({ amount, method }) => {
      const tx: Transaction = {
        id: `t_${Date.now()}`,
        type: "received",
        name: method,
        username: `@${method.toLowerCase().replace(" ", "")}`,
        avatar: "https://images.unsplash.com/photo-1621761191319-c6fb62004040?auto=format&fit=crop&q=80&w=200&h=200",
        amount,
        note: `Deposited via ${method}`,
        date: "Just now",
        status: "completed",
      };
      updatePortfolio((current) => ({
        ...current,
        balance: current.balance + amount,
        transactions: [tx, ...current.transactions],
      }));
    },
    onError: (error) => {
      console.error(error);
    }
  });

  const deposit = (amount: number, method: string) => {
    depositMutation.mutate({ amount, method });
  };

  const withdrawMutation = useMutation({
    mutationFn: async ({ amount, bank }: { amount: number, bank: string }) => {
      const res = await fetch(`${API_BASE_URL}/api/v1/anchor/withdraw`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          anchor_domain: bank,
          asset_code: "USDC",
          account: initialUser.stellarAddress
        })
      });
      if (!res.ok) throw new Error("Withdrawal failed");
      return { amount, bank };
    },
    onSuccess: ({ amount, bank }) => {
      const tx: Transaction = {
        id: `t_${Date.now()}`,
        type: "sent",
        name: bank,
        username: `@${bank.toLowerCase().replace(" ", "")}`,
        avatar: "https://images.unsplash.com/photo-1621761191319-c6fb62004040?auto=format&fit=crop&q=80&w=200&h=200",
        amount,
        note: `Withdrew to ${bank}`,
        date: "Just now",
        status: "completed",
      };
      updatePortfolio((current) => ({
        ...current,
        balance: current.balance - amount,
        transactions: [tx, ...current.transactions],
      }));
    },
    onError: (error) => {
      console.error(error);
    }
  });

  const withdraw = (amount: number, bank: string) => {
    withdrawMutation.mutate({ amount, bank });
  };

  return (
    <WalletContext.Provider value={{ 
      balance, transactions, addTransaction, deposit, withdraw,
      isDepositing: depositMutation.isPending, 
      isWithdrawing: withdrawMutation.isPending,
      isOffline,
      isPortfolioUnavailable,
      refreshPortfolio: async () => {
        await portfolioQuery.refetch();
      },
    }}>
      {children}
    </WalletContext.Provider>
  );
};

export const useWallet = () => {
  const context = useContext(WalletContext);
  if (!context) throw new Error("useWallet must be used within a WalletProvider");
  return context;
};
