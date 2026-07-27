import { useAccount, useConnect, useDisconnect, useBalance } from "wagmi";
import { useWalletConnection } from "../useWalletConnection";

jest.mock("wagmi", () => ({
  useAccount: jest.fn(),
  useConnect: jest.fn(),
  useDisconnect: jest.fn(),
  useBalance: jest.fn(),
}));

describe("useWalletConnection Hook", () => {
  const mockConnect = jest.fn();
  const mockDisconnect = jest.fn();
  const mockRefetchBalance = jest.fn();

  beforeEach(() => {
    jest.clearAllMocks();

    (useAccount as jest.Mock).mockReturnValue({
      address: "0x1234567890123456789012345678901234567890",
      isConnected: true,
      isConnecting: false,
      chain: { id: 11155111, name: "Sepolia" },
    });

    (useConnect as jest.Mock).mockReturnValue({
      connect: mockConnect,
      connectors: [{ id: "injected", name: "Injected" }],
      error: null,
    });

    (useDisconnect as jest.Mock).mockReturnValue({
      disconnect: mockDisconnect,
      error: null,
    });

    (useBalance as jest.Mock).mockReturnValue({
      data: { value: 1000000000000000000n, decimals: 18, symbol: "ETH" },
      isLoading: false,
      refetch: mockRefetchBalance,
      error: null,
    });
  });

  it("should return the connection status and address", () => {
    const result = useWalletConnection();
    expect(result.address).toBe("0x1234567890123456789012345678901234567890");
    expect(result.isConnected).toBe(true);
    expect(result.isConnecting).toBe(false);
    expect(result.chain?.id).toBe(11155111);
  });

  it("should return the balance data", () => {
    const result = useWalletConnection();
    expect(result.balance?.symbol).toBe("ETH");
    expect(result.balance?.value).toBe(1000000000000000000n);
    expect(result.isLoadingBalance).toBe(false);
  });

  it("should call connect with correct parameter shape", () => {
    const result = useWalletConnection();
    result.connect("mockConnector");
    expect(mockConnect).toHaveBeenCalledWith({ connector: "mockConnector" });
  });

  it("should call disconnect", () => {
    const result = useWalletConnection();
    result.disconnect();
    expect(mockDisconnect).toHaveBeenCalled();
  });

  it("should call refetchBalance", () => {
    const result = useWalletConnection();
    result.refetchBalance();
    expect(mockRefetchBalance).toHaveBeenCalled();
  });

  it("should aggregate errors from connection, disconnection, and balance fetching", () => {
    const mockConnectError = new Error("Connection failed");
    const mockDisconnectError = new Error("Disconnection failed");
    const mockBalanceError = new Error("Balance fetch failed");

    (useConnect as jest.Mock).mockReturnValue({
      connect: mockConnect,
      connectors: [],
      error: mockConnectError,
    });

    (useDisconnect as jest.Mock).mockReturnValue({
      disconnect: mockDisconnect,
      error: mockDisconnectError,
    });

    (useBalance as jest.Mock).mockReturnValue({
      data: undefined,
      isLoading: false,
      refetch: mockRefetchBalance,
      error: mockBalanceError,
    });

    const result = useWalletConnection();
    // It should prioritize/return the connectError or fall back
    expect(result.error).toBe(mockConnectError);
  });
});
