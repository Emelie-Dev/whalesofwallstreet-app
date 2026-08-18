import React, { createContext, useContext, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { currentUser as initialUser, initialTransactions, type Transaction } from "../data/mockData";

type WalletContextType = {
  balance: number;
  transactions: Transaction[];
  addTransaction: (tx: Omit<Transaction, "id" | "date" | "status">) => void;
  deposit: (amount: number, method: string) => void;
  withdraw: (amount: number, bank: string) => void;
  isDepositing: boolean;
  isWithdrawing: boolean;
};

const WalletContext = createContext<WalletContextType | undefined>(undefined);

const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || "http://localhost:8080";

export const WalletProvider = ({ children }: { children: React.ReactNode }) => {
  const [balance, setBalance] = useState(initialUser.balance);
  const [transactions, setTransactions] = useState<Transaction[]>(initialTransactions);

  const addTransaction = (newTx: Omit<Transaction, "id" | "date" | "status">) => {
    const tx: Transaction = {
      ...newTx,
      id: `t_${Date.now()}`,
      date: "Just now",
      status: "completed",
    };
    setTransactions((prev) => [tx, ...prev]);
    if (newTx.type === "sent") {
      setBalance((prev) => prev - newTx.amount);
    } else {
      setBalance((prev) => prev + newTx.amount);
    }
  };

  const depositMutation = useMutation({
    mutationFn: async ({ amount, method }: { amount: number, method: string }) => {
      const res = await fetch(`${API_BASE_URL}/api/v1/anchor/deposit`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          anchor_domain: method,
          asset_code: "USDC",
          account: (initialUser as any).stellarAddress || "GA5Z3IX5VQ3N6FB77T342A27RWRN7CKEZ63M3W7S5VJB3D77J6F2JAFK"
        })
      });
      if (!res.ok) throw new Error("Deposit failed");
      return { amount, method };
    },
    onMutate: ({ amount, method }) => {
      const previousBalance = balance;
      const previousTransactions = transactions;
      const optimisticId = `pending_${Date.now()}`;

      const optimisticTx: Transaction = {
        id: optimisticId,
        type: "received",
        name: method,
        username: `@${method.toLowerCase().replace(" ", "")}`,
        avatar: "https://images.unsplash.com/photo-1621761191319-c6fb62004040?auto=format&fit=crop&q=80&w=200&h=200",
        amount,
        note: `Depositing via ${method}`,
        date: "Just now",
        status: "pending",
      };

      setTransactions((prev) => [optimisticTx, ...prev]);
      setBalance((prev) => prev + amount);

      return { previousBalance, previousTransactions, optimisticId };
    },
    onSuccess: (_result, _variables, context) => {
      // Flip the optimistic entry to completed in place instead of
      // appending a second transaction for the same deposit.
      setTransactions((prev) =>
        prev.map((tx) =>
          tx.id === context?.optimisticId ? { ...tx, status: "completed" } : tx
        )
      );
    },
    onError: (error, _variables, context) => {
      console.error(error);
      // Roll back to the pre-mutation snapshot: the bridge transaction
      // reverted, so the optimistic balance/transaction never happened.
      if (context) {
        setBalance(context.previousBalance);
        setTransactions(context.previousTransactions);
      }
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
          account: (initialUser as any).stellarAddress || "GA5Z3IX5VQ3N6FB77T342A27RWRN7CKEZ63M3W7S5VJB3D77J6F2JAFK"
        })
      });
      if (!res.ok) throw new Error("Withdrawal failed");
      return { amount, bank };
    },
    onMutate: ({ amount, bank }) => {
      const previousBalance = balance;
      const previousTransactions = transactions;
      const optimisticId = `pending_${Date.now()}`;

      const optimisticTx: Transaction = {
        id: optimisticId,
        type: "sent",
        name: bank,
        username: `@${bank.toLowerCase().replace(" ", "")}`,
        avatar: "https://images.unsplash.com/photo-1621761191319-c6fb62004040?auto=format&fit=crop&q=80&w=200&h=200",
        amount,
        note: `Withdrawing to ${bank}`,
        date: "Just now",
        status: "pending",
      };

      setTransactions((prev) => [optimisticTx, ...prev]);
      setBalance((prev) => prev - amount);

      return { previousBalance, previousTransactions, optimisticId };
    },
    onSuccess: (_result, _variables, context) => {
      setTransactions((prev) =>
        prev.map((tx) =>
          tx.id === context?.optimisticId ? { ...tx, status: "completed" } : tx
        )
      );
    },
    onError: (error, _variables, context) => {
      console.error(error);
      if (context) {
        setBalance(context.previousBalance);
        setTransactions(context.previousTransactions);
      }
    }
  });

  const withdraw = (amount: number, bank: string) => {
    withdrawMutation.mutate({ amount, bank });
  };

  return (
    <WalletContext.Provider value={{ 
      balance, transactions, addTransaction, deposit, withdraw, 
      isDepositing: depositMutation.isPending, 
      isWithdrawing: withdrawMutation.isPending 
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
