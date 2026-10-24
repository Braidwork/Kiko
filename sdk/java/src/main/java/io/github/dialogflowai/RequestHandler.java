package io.github.braidwork.kiko;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.MappingIterator;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.github.braidwork.kiko.sdk.Answer;
import io.github.braidwork.kiko.sdk.RequestData;
import io.github.braidwork.kiko.sdk.Response;
import io.github.braidwork.kiko.sdk.StreamingResponseData;
import io.github.braidwork.kiko.sdk.UserInputResult;
import lombok.extern.slf4j.Slf4j;

import java.io.IOException;
import java.io.InputStream;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

@Slf4j
public class RequestHandler {
    /** The framing a streamed answer uses: one JSON document per line. */
    private static final String NDJSON = "application/x-ndjson";

    /**
     * What a streamed answer's text is labelled with. Frames carry text and
     * nothing else, so an answer rebuilt from them is always reported as plain
     * text, even when the node that produced it would have emitted HTML.
     */
    private static final String TEXT_PLAIN = "TextPlain";

    private final HttpClient client;
    private final URI endpoint;
    private final ObjectMapper mapper;
    private volatile long idleTimeoutMillis = 0;

    public RequestHandler(String endpoint) {
        this.client = HttpClient.newBuilder()
                .version(HttpClient.Version.HTTP_1_1)
                .build();
        this.endpoint = URI.create(endpoint);
        this.mapper = new ObjectMapper();
        this.mapper.configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false);
        System.setProperty("jdk.httpclient.keepalive.timeout", "1800");
    }

    public Response req(String robotId, String mainFlowId, String userInput) throws IOException, InterruptedException {
        return req(userInput, robotId, mainFlowId, 2500);
    }

    public Response req(String robotId, String mainFlowId, String userInput, int timeoutMillis) throws IOException, InterruptedException {
        RequestData requestData = RequestData.create(robotId, mainFlowId, userInput);
        return req(requestData, timeoutMillis);
    }

    public Response req(RequestData requestData, int timeoutMillis) throws IOException, InterruptedException {
        return req(requestData, timeoutMillis, null);
    }

    /**
     * Sends the request and hands each answer delta to {@code onChunk} as it
     * arrives, instead of waiting for the answer to be complete.
     *
     * <p>Passing a callback is what asks for a stream: this sets
     * {@link RequestData#setStream(boolean) stream} on the request, so a caller
     * cannot pass a callback and silently get a buffered answer back.
     *
     * <p>The division of labour is the server's, mirrored here: a streamed answer
     * reaches you through {@code onChunk} and is <em>not</em> part of
     * {@link Response#getData() data.answers}, which keeps only the answers that
     * were never streamed. Nothing is delivered twice and nothing is dropped, so
     * a caller that wants the whole transcript accumulates in the callback —
     * which is also what lets it print tokens as they are produced.
     *
     * <p>Without a callback the method blocks until the answer is complete and
     * the returned response always holds the whole thing, streamed or not.
     *
     * @param onChunk called once per answer delta, in arrival order. The terminal
     *                frame is not a delta — it carries the response document
     *                rather than answer text — so it is parsed into the returned
     *                response instead of being handed over here, which is what
     *                lets a caller print {@code chunk.getContent()} as-is. May be
     *                {@code null}.
     * @return the response, whose answers were merged from the deltas whenever no
     *         callback was given
     */
    public Response req(RequestData requestData, int timeoutMillis, Consumer<StreamingResponseData> onChunk) throws IOException, InterruptedException {
        // Check default value of request params
        if (requestData.getUserInput() == null)
            requestData.setUserInput("");
        if (requestData.getUserInputResult() == null)
            requestData.setUserInputResult(UserInputResult.SUCCESSFUL);
        if (onChunk != null)
            requestData.setStream(true);
        // End
        return post(requestData, timeoutMillis, onChunk);
    }

    /**
     * Milliseconds without a frame after which a streamed answer is abandoned,
     * or {@code 0} — the default — to wait indefinitely.
     *
     * <p>{@link HttpRequest.Builder#timeout(Duration)} stops applying as soon as
     * the response headers have been read, so it bounds nothing here: with
     * {@link HttpResponse.BodyHandlers#ofInputStream()} the body is read after
     * that point, and a server that stops sending frames mid-answer would park
     * the calling thread for good. This is the only guard against that.
     */
    public void setIdleTimeoutMillis(long idleTimeoutMillis) {
        this.idleTimeoutMillis = idleTimeoutMillis;
    }

    private Response post(RequestData requestData, int timeoutMillis, Consumer<StreamingResponseData> onChunk) throws IOException, InterruptedException {
        HttpRequest request = HttpRequest.newBuilder()
                .uri(endpoint)
                .timeout(Duration.ofMillis(timeoutMillis))
                .POST(HttpRequest.BodyPublishers.ofByteArray(mapper.writeValueAsBytes(requestData)))
                .header("Content-Type", "application/json")
                .build();

        // Send request and handle response
        HttpResponse<InputStream> response = client.send(request, HttpResponse.BodyHandlers.ofInputStream());
        try (InputStream body = response.body()) {
            if (isNdjson(response)) {
                return readFrames(body, onChunk);
            }
            return mapper.readValue(body, Response.class);
        }
    }

    private static boolean isNdjson(HttpResponse<?> response) {
        return response.headers().firstValue("Content-Type")
                .map(value -> value.toLowerCase(Locale.ROOT).contains(NDJSON))
                .orElse(false);
    }

    /**
     * Reads a streamed answer, frame by frame.
     *
     * <p>Jackson walks the body as a sequence of root values, so the framing is
     * the server's business: it does not matter where a TCP chunk happens to
     * split, how many frames one read returns, or whether the body arrives all at
     * once.
     */
    private Response readFrames(InputStream body, Consumer<StreamingResponseData> onChunk) throws IOException {
        Map<Integer, StringBuilder> deltas = new TreeMap<>();
        String envelope = null;
        IdleWatchdog watchdog = new IdleWatchdog(body, idleTimeoutMillis);
        try (MappingIterator<StreamingResponseData> frames =
                     mapper.readerFor(StreamingResponseData.class).readValues(body)) {
            while (frames.hasNextValue()) {
                StreamingResponseData frame = frames.nextValue();
                watchdog.pet();
                Integer seq = frame.getContentSeq();
                if (seq == null) {
                    // Terminal frame: the payload is the response document, and
                    // nothing may follow it.
                    envelope = frame.getContent();
                    break;
                }
                deltas.computeIfAbsent(seq, key -> new StringBuilder())
                        .append(frame.getContent() == null ? "" : frame.getContent());
                if (onChunk != null) {
                    onChunk.accept(frame);
                }
            }
        } catch (IOException e) {
            throw watchdog.explain(e);
        } finally {
            watchdog.shutdown();
        }
        if (envelope == null) {
            throw new IOException("The answer stream ended without a terminal frame.");
        }
        Response response = mapper.readValue(envelope, Response.class);
        if (onChunk == null) {
            mergeDeltas(response, deltas);
        }
        return response;
    }

    /**
     * Rebuilds the answers out of the deltas.
     *
     * <p>Only for the callers that took no callback: their answer arrived as
     * frames, so {@code data.answers} as sent is empty and the text would be lost
     * — which is what a plain {@code readValue} on a streamed body used to do,
     * silently returning a 200 with nothing in it.
     */
    private static void mergeDeltas(Response response, Map<Integer, StringBuilder> deltas) {
        if (deltas.isEmpty() || response.getData() == null) {
            return;
        }
        List<Answer> streamed = new ArrayList<>(deltas.size());
        for (StringBuilder text : deltas.values()) {
            Answer answer = new Answer();
            answer.setContent(text.toString());
            answer.setContentType(TEXT_PLAIN);
            streamed.add(answer);
        }
        List<Answer> buffered = response.getData().getAnswers();
        if (buffered == null || buffered.isEmpty()) {
            response.getData().setAnswers(streamed);
            return;
        }
        // A streamed request pushes every answer as a frame, so this should not
        // happen. Keep both rather than dropping either.
        log.warn("Response carried {} buffered answer(s) next to {} streamed one(s).",
                buffered.size(), streamed.size());
        List<Answer> all = new ArrayList<>(buffered.size() + streamed.size());
        all.addAll(buffered);
        all.addAll(streamed);
        response.getData().setAnswers(all);
    }

    /**
     * Fails a stalled stream back to the caller.
     *
     * <p>Closing the body is what unblocks a pending read: the JDK's body stream
     * offers a sentinel buffer to the queue its reader is parked on before
     * returning from {@code close()}, so the read wakes up and throws. A no-op
     * unless {@link #setIdleTimeoutMillis} was set.
     */
    private final class IdleWatchdog {
        private final InputStream body;
        private final long timeoutNanos;
        private final ScheduledExecutorService scheduler;
        private volatile long lastFrameNanos = System.nanoTime();
        private volatile boolean tripped;

        private IdleWatchdog(InputStream body, long timeoutMillis) {
            this.body = body;
            this.timeoutNanos = TimeUnit.MILLISECONDS.toNanos(timeoutMillis);
            if (timeoutMillis <= 0) {
                this.scheduler = null;
                return;
            }
            this.scheduler = Executors.newSingleThreadScheduledExecutor(runnable -> {
                Thread thread = new Thread(runnable, "kiko-idle-watchdog");
                thread.setDaemon(true);
                return thread;
            });
            // Polling rather than one timer per frame: an answer produces a frame
            // per token, and rescheduling that often costs more than it is worth.
            long period = Math.max(1, timeoutMillis / 4);
            this.scheduler.scheduleAtFixedRate(this::check, period, period, TimeUnit.MILLISECONDS);
        }

        private void pet() {
            lastFrameNanos = System.nanoTime();
        }

        private void check() {
            if (tripped || System.nanoTime() - lastFrameNanos < timeoutNanos) {
                return;
            }
            tripped = true;
            log.warn("No frame for {} ms, abandoning the stream.",
                    TimeUnit.NANOSECONDS.toMillis(timeoutNanos));
            try {
                body.close();
            } catch (IOException e) {
                log.debug("Closing an idle body failed.", e);
            }
        }

        /** Replaces the cryptic "closed" the wake-up causes with the real reason. */
        private IOException explain(IOException e) {
            if (!tripped) {
                return e;
            }
            return new IOException("No frame received for " + idleTimeoutMillis + " ms.", e);
        }

        private void shutdown() {
            if (scheduler != null) {
                scheduler.shutdownNow();
            }
        }
    }
}
