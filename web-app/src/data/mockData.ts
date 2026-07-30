export const currentUser = {
  name: "Victor",
  username: "@victor",
  email: "victor@email.com",
  avatar: "https://images.unsplash.com/photo-1534528741775-53994a69daeb?auto=format&fit=crop&q=80&w=200&h=200",
  balance: 1240.5,
};

export const contacts = [
  { id: "1", name: "Sarah", username: "@sarah", avatar: "https://images.unsplash.com/photo-1494790108377-be9c29b29330?auto=format&fit=crop&q=80&w=200&h=200" },
  { id: "2", name: "Alex", username: "@alex", avatar: "https://images.unsplash.com/photo-1507003211169-0a1dd7228f2d?auto=format&fit=crop&q=80&w=200&h=200" },
  { id: "3", name: "Maya", username: "@maya", avatar: "https://images.unsplash.com/photo-1438761681033-6461ffad8d80?auto=format&fit=crop&q=80&w=200&h=200" },
  { id: "4", name: "James", username: "@james", avatar: "https://images.unsplash.com/photo-1500648767791-00dcc994a43e?auto=format&fit=crop&q=80&w=200&h=200" },
  { id: "5", name: "Lily", username: "@lily", avatar: "https://images.unsplash.com/photo-1544005313-94ddf0286df2?auto=format&fit=crop&q=80&w=200&h=200" },
  { id: "6", name: "Omar", username: "@omar", avatar: "https://images.unsplash.com/photo-1522075469751-3a6694fb2f61?auto=format&fit=crop&q=80&w=200&h=200" },
];

export type Transaction = {
  id: string;
  type: "sent" | "received";
  name: string;
  username: string;
  avatar: string;
  amount: number;
  note?: string;
  date: string;
  status: "completed" | "pending";
};

// Generate large transaction history for testing virtualization
const generateLargeTransactionHistory = (): Transaction[] => {
  const transactions: Transaction[] = [
    { id: "t1", type: "received", name: "Sarah", username: "@sarah", avatar: "https://images.unsplash.com/photo-1494790108377-be9c29b29330?auto=format&fit=crop&q=80&w=200&h=200", amount: 50.0, note: "Dinner split", date: "Today, 2:30 PM", status: "completed" },
    { id: "t2", type: "sent", name: "Alex", username: "@alex", avatar: "https://images.unsplash.com/photo-1507003211169-0a1dd7228f2d?auto=format&fit=crop&q=80&w=200&h=200", amount: 25.0, note: "Coffee ☕", date: "Today, 11:15 AM", status: "completed" },
    { id: "t3", type: "received", name: "Maya", username: "@maya", avatar: "https://images.unsplash.com/photo-1438761681033-6461ffad8d80?auto=format&fit=crop&q=80&w=200&h=200", amount: 120.0, note: "Freelance project", date: "Yesterday", status: "completed" },
    { id: "t4", type: "sent", name: "James", username: "@james", avatar: "https://images.unsplash.com/photo-1500648767791-00dcc994a43e?auto=format&fit=crop&q=80&w=200&h=200", amount: 15.0, date: "Yesterday", status: "pending" },
    { id: "t5", type: "received", name: "Lily", username: "@lily", avatar: "https://images.unsplash.com/photo-1544005313-94ddf0286df2?auto=format&fit=crop&q=80&w=200&h=200", amount: 200.0, note: "Rent contribution", date: "Feb 10", status: "completed" },
    { id: "t6", type: "sent", name: "Omar", username: "@omar", avatar: "https://images.unsplash.com/photo-1522075469751-3a6694fb2f61?auto=format&fit=crop&q=80&w=200&h=200", amount: 8.5, note: "Snacks", date: "Feb 9", status: "completed" },
  ];

  // Generate 1000+ additional transactions to simulate power user scenario
  const names = ["Sarah", "Alex", "Maya", "James", "Lily", "Omar", "Emma", "Noah", "Olivia", "Liam", "Ava", "William", "Sophia", "James", "Isabella"];
  const notes = ["Payment", "Transfer", "Gift", "Refund", "Service", "Product", "Subscription", "Loan", "Investment", "Dividend"];
  const avatars = [
    "https://images.unsplash.com/photo-1494790108377-be9c29b29330?auto=format&fit=crop&q=80&w=200&h=200",
    "https://images.unsplash.com/photo-1507003211169-0a1dd7228f2d?auto=format&fit=crop&q=80&w=200&h=200",
    "https://images.unsplash.com/photo-1438761681033-6461ffad8d80?auto=format&fit=crop&q=80&w=200&h=200",
    "https://images.unsplash.com/photo-1500648767791-00dcc994a43e?auto=format&fit=crop&q=80&w=200&h=200",
    "https://images.unsplash.com/photo-1544005313-94ddf0286df2?auto=format&fit=crop&q=80&w=200&h=200",
    "https://images.unsplash.com/photo-1522075469751-3a6694fb2f61?auto=format&fit=crop&q=80&w=200&h=200",
  ];

  for (let i = 7; i <= 1000; i++) {
    const isReceived = Math.random() > 0.5;
    const name = names[Math.floor(Math.random() * names.length)];
    const note = notes[Math.floor(Math.random() * notes.length)];
    const avatar = avatars[Math.floor(Math.random() * avatars.length)];
    const amount = Math.round((Math.random() * 500 + 5) * 100) / 100;
    const daysAgo = Math.floor(Math.random() * 365);
    const date = daysAgo === 0 ? "Today" : daysAgo === 1 ? "Yesterday" : `${daysAgo} days ago`;
    
    transactions.push({
      id: `t${i}`,
      type: isReceived ? "received" : "sent",
      name,
      username: `@${name.toLowerCase()}`,
      avatar,
      amount,
      note,
      date,
      status: Math.random() > 0.9 ? "pending" : "completed",
    });
  }

  return transactions;
};

export const initialTransactions: Transaction[] = generateLargeTransactionHistory();
