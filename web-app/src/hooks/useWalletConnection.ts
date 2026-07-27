import { useAccount, useConnect, useDisconnect, useBalance } from "wagmi";

export const useWalletConnection = () => {
  const { address, isConnected, isConnecting, chain } = useAccount();
  const { connect, connectors, error: connectError } = useConnect();
  const { disconnect, error: disconnectError } = useDisconnect();

  // Fetch balance for the connected address
  const { 
    data: balance, 
    isLoading: isLoadingBalance, 
    refetch: refetchBalance,
    error: balanceError 
  } = useBalance({
    address,
  });

  // Combine errors if any occur during connection, disconnection, or fetching balance
  const error = connectError || disconnectError || balanceError || null;

  return {
    address,
    isConnected,
    isConnecting,
    connectors,
    connect: (connector: any) => connect({ connector }),
    disconnect,
    error,
    chain,
    balance,
    isLoadingBalance,
    refetchBalance,
  };
};
