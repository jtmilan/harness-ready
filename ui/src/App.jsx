import { lazy, Suspense } from 'react'
import { Toaster } from "@/components/ui/toaster"
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClientInstance } from '@/lib/query-client'
import { BrowserRouter as Router, Route, Routes } from 'react-router-dom';
import ScrollToTop from './components/ScrollToTop';
// Route-level code-split: each page is its own async chunk (Home still pulls the
// bulk of the terminal UI; Monitoring/404 stay off the initial critical path).
const Home = lazy(() => import('./pages/Home'));
const Monitoring = lazy(() => import('./pages/Monitoring'));
const PageNotFound = lazy(() => import('./lib/PageNotFound'));

function App() {
  return (
    <QueryClientProvider client={queryClientInstance}>
      <Router>
        <ScrollToTop />
        <Suspense fallback={null}>
          <Routes>
            <Route path="/" element={<Home />} />
            <Route path="/monitoring" element={<Monitoring />} />
            {/* Add your page Route elements here */}
            <Route path="*" element={<PageNotFound />} />
          </Routes>
        </Suspense>
      </Router>
      <Toaster />
    </QueryClientProvider>
  )
}

export default App
