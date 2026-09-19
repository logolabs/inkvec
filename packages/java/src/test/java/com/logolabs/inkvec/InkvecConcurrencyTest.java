package com.logolabs.inkvec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

/**
 * Every entry point may be called from any number of threads at once (docs/BINDINGS.md,
 * "Threading"): the native pipeline is itself parallel, one global pool per process, and the
 * C ABI is re-entrant. This drives many concurrent traces -- of both input forms, and both
 * a success path and an error path -- through one loaded {@link Inkvec} and checks nothing
 * corrupts another call's {@link InkvecResultStruct} or throws unexpectedly.
 */
final class InkvecConcurrencyTest {

    @Test
    void concurrentTracesAreThreadSafeAndDeterministic() throws InterruptedException, ExecutionException {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        byte[] rgba = ContractSupport.input(ContractSupport.caseByName("tiny_rgba_defaults"));
        String expected = Inkvec.trace(png).svg();

        int threads = 12;
        int perThread = 8;
        ExecutorService pool = Executors.newFixedThreadPool(threads);
        AtomicInteger errorPathHits = new AtomicInteger();
        try {
            List<Callable<String>> tasks = new ArrayList<>();
            for (int t = 0; t < threads; t++) {
                final int id = t;
                tasks.add(
                        () -> {
                            for (int i = 0; i < perThread; i++) {
                                // Interleave the error path on some threads: a failed call
                                // must not corrupt the InkvecResultStruct another thread is
                                // using at the same moment.
                                if (id % 3 == 0) {
                                    try {
                                        Inkvec.trace(new byte[] {9, 9, 9});
                                    } catch (InvalidImageException expectedFailure) {
                                        errorPathHits.incrementAndGet();
                                    }
                                }
                                TraceResult a = Inkvec.trace(png);
                                TraceResult b = Inkvec.traceRgba(rgba, 96, 96);
                                if (!a.svg().equals(b.svg())) {
                                    throw new AssertionError("encoded/rgba diverged on thread " + id);
                                }
                            }
                            return Inkvec.trace(png).svg();
                        });
            }
            List<Future<String>> futures = pool.invokeAll(tasks);
            Set<String> distinctResults = new java.util.HashSet<>();
            for (Future<String> f : futures) {
                distinctResults.add(f.get());
            }
            assertEquals(1, distinctResults.size(), "every thread must get the same bytes");
            assertEquals(expected, distinctResults.iterator().next());
            assertTrue(errorPathHits.get() > 0, "the error path should have run");
        } finally {
            pool.shutdown();
            assertTrue(pool.awaitTermination(30, TimeUnit.SECONDS));
        }
    }
}
