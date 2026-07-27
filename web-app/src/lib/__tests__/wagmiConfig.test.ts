import { createConfig } from "wagmi";
import { config } from "../wagmiConfig";

jest.mock("wagmi", () => ({
  createConfig: jest.fn((options) => ({
    chains: options.chains,
    connectors: options.connectors,
  })),
  http: jest.fn(),
}));

jest.mock("wagmi/chains", () => ({
  mainnet: { id: 1, name: "Ethereum" },
  sepolia: { id: 11155111, name: "Sepolia" },
  arbitrum: { id: 42161, name: "Arbitrum" },
  arbitrumSepolia: { id: 421614, name: "Arbitrum Sepolia" },
}));

jest.mock("wagmi/connectors", () => ({
  injected: jest.fn().mockReturnValue({ name: "Injected" }),
  walletConnect: jest.fn().mockReturnValue({ name: "WalletConnect" }),
}));

describe("Wagmi Config", () => {
  it("should create config successfully", () => {
    expect(config).toBeDefined();
    expect(createConfig).toHaveBeenCalled();
  });

  it("should have chains configured", () => {
    expect(config.chains).toBeDefined();
    expect(config.chains.length).toBe(4);
    
    const chainIds = config.chains.map((c: any) => c.id);
    expect(chainIds).toContain(1); // Mainnet
    expect(chainIds).toContain(11155111); // Sepolia
    expect(chainIds).toContain(42161); // Arbitrum
    expect(chainIds).toContain(421614); // Arbitrum Sepolia
  });

  it("should have connectors configured", () => {
    expect(config.connectors).toBeDefined();
    expect(config.connectors.length).toBe(2);
  });
});
