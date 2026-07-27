import { http, createConfig } from "wagmi";
import { mainnet, sepolia, arbitrum, arbitrumSepolia } from "wagmi/chains";
import { injected, walletConnect } from "wagmi/connectors";

// Declared globally for TypeScript compilation safety
declare const __WALLETCONNECT_PROJECT_ID__: string;

const projectId = typeof __WALLETCONNECT_PROJECT_ID__ !== "undefined"
  ? __WALLETCONNECT_PROJECT_ID__
  : "3fcc6b1f64f43c3f25c7e090f7777777";

export const config = createConfig({
  chains: [mainnet, sepolia, arbitrum, arbitrumSepolia],
  connectors: [
    injected(),
    walletConnect({
      projectId,
      showQrModal: true,
      metadata: {
        name: "Wow App",
        description: "Production Wallet Integration for Wow App",
        url: typeof window !== "undefined" ? window.location.origin : "https://localhost:3000",
        icons: ["https://images.unsplash.com/photo-1621761191319-c6fb62004040?auto=format&fit=crop&q=80&w=200&h=200"],
      },
    }),
  ],
  transports: {
    [mainnet.id]: http(),
    [sepolia.id]: http(),
    [arbitrum.id]: http(),
    [arbitrumSepolia.id]: http(),
  },
});
