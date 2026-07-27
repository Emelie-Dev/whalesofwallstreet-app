export default {
  preset: 'ts-jest',
  testEnvironment: 'node',
  globals: {
    __WALLETCONNECT_PROJECT_ID__: '3fcc6b1f64f43c3f25c7e090f7777777',
  },
  testPathIgnorePatterns: ['/node_modules/', '/native-app/'],
  transformIgnorePatterns: [
    '/node_modules/(?!(wagmi|@wagmi|viem|@viem|abitype|@adraffy|@noble|@scure)/)'
  ],
  transform: {
    '^.+\\.tsx?$': ['ts-jest', { 
      useESM: true,
      diagnostics: {
        ignoreCodes: [1343]
      },
      tsconfig: {
        jsx: 'react-jsx',
        module: 'esnext',
        target: 'es2020',
        esModuleInterop: true,
        allowSyntheticDefaultImports: true,
        types: ['jest', 'node']
      }
    }],
  },
  extensionsToTreatAsEsm: ['.ts', '.tsx'],
};
