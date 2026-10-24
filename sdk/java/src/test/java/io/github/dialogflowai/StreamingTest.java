package io.github.braidwork.kiko;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import io.github.braidwork.kiko.sdk.Answer;
import io.github.braidwork.kiko.sdk.NextAction;
import io.github.braidwork.kiko.sdk.RequestData;
import io.github.braidwork.kiko.sdk.Response;
import io.github.braidwork.kiko.sdk.ResponseData;
import io.github.braidwork.kiko.sdk.StreamingResponseData;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.io.OutputStream;
import java.io.UncheckedIOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Consumer;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Exercises the streaming protocol against a local server that writes frames by
 * hand, so the framing, the aggregation and the stalled-stream guard are all
 * covered without a running dialogflow-rs instance.
 */
public class StreamingTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    /** The two deltas of answer 0, then a whole second answer, then the terminal frame. */
    private static final String[] FRAMES = {
            "{\"contentSeq\":0,\"content\":\"Hello, \"}",
            "{\"contentSeq\":0,\"content\":\"world\"}",
            "{\"contentSeq\":1,\"content\":\"Second answer\"}",
    };

    private static HttpServer serve(Consumer<HttpExchange> handler) throws IOException {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            try {
                handler.accept(exchange);
            } finally {
                exchange.close();
            }
        });
        // Without this the handlers share one dispatcher thread, and the stalled
        // server below would block every other test.
        server.setExecutor(Executors.newCachedThreadPool());
        server.start();
        return server;
    }

    private static String url(HttpServer server) {
        return "http://127.0.0.1:" + server.getAddress().getPort() + "/flow/answer";
    }

    /** Opens an NDJSON response and hands the body back for the caller to write. */
    private static OutputStream startNdjson(HttpExchange exchange) throws IOException {
        exchange.getResponseHeaders().add("Content-Type", "application/x-ndjson");
        exchange.sendResponseHeaders(200, 0);
        return exchange.getResponseBody();
    }

    private static void write(OutputStream out, String line) throws IOException {
        out.write((line + "\n").getBytes(StandardCharsets.UTF_8));
        out.flush();
    }

    private static String terminalFrame(NextAction nextAction) throws IOException {
        ResponseData data = new ResponseData();
        data.setSessionId("s1");
        // The server leaves this empty for a streamed answer: the text went out
        // as frames, not in the document.
        data.setAnswers(new ArrayList<>());
        data.setCollectData(new ArrayList<>());
        data.setNextAction(nextAction);
        Response envelope = new Response();
        envelope.setStatus(200);
        envelope.setData(data);
        StreamingResponseData terminal = new StreamingResponseData();
        // A null sequence is what marks this frame as the end of the stream, and
        // its content is the response document rather than more answer text.
        terminal.setContentSeq(null);
        terminal.setContent(MAPPER.writeValueAsString(envelope));
        return MAPPER.writeValueAsString(terminal);
    }

    @Test
    void the_stream_flag_is_always_serialized() throws IOException {
        RequestData off = RequestData.create("r", "f");
        RequestData on = RequestData.create("r", "f");
        on.setStream(true);
        // A primitive, so the request states which of the two it wants instead of
        // leaving the server to guess from an absent field.
        assertTrue(MAPPER.writeValueAsString(off).contains("\"stream\":false"),
                MAPPER.writeValueAsString(off));
        assertTrue(MAPPER.writeValueAsString(on).contains("\"stream\":true"),
                MAPPER.writeValueAsString(on));
    }

    @Test
    void a_plain_json_answer_still_reads_as_before() throws Exception {
        HttpServer server = serve(exchange -> {
            try {
                ResponseData data = new ResponseData();
                data.setSessionId("s1");
                List<Answer> answers = new ArrayList<>();
                Answer answer = new Answer();
                answer.setContent("plain");
                answer.setContentType("TextPlain");
                answers.add(answer);
                data.setAnswers(answers);
                data.setNextAction(NextAction.TERMINATE);
                Response envelope = new Response();
                envelope.setStatus(200);
                envelope.setData(data);
                byte[] body = MAPPER.writeValueAsBytes(envelope);
                exchange.getResponseHeaders().add("Content-Type", "application/json");
                exchange.sendResponseHeaders(200, body.length);
                exchange.getResponseBody().write(body);
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        });
        try {
            RequestData request = RequestData.create("r", "f", "hi");
            Response response = new RequestHandler(url(server)).req(request, 5000);
            assertEquals(200, response.getStatus());
            assertEquals("plain", response.getData().getAnswers().get(0).getContent());
            assertEquals(NextAction.TERMINATE, response.getData().getNextAction());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void deltas_are_merged_when_no_callback_is_given() throws Exception {
        HttpServer server = serve(exchange -> {
            try {
                OutputStream out = startNdjson(exchange);
                for (String frame : FRAMES) {
                    write(out, frame);
                }
                write(out, terminalFrame(NextAction.TERMINATE));
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        });
        try {
            RequestData request = RequestData.create("r", "f", "hi");
            request.setStream(true);
            Response response = new RequestHandler(url(server)).req(request, 5000);

            assertEquals(200, response.getStatus());
            assertEquals(NextAction.TERMINATE, response.getData().getNextAction());
            // Without the merge a streamed answer reads as a 200 with nothing in
            // it, which is what a bare readValue on this body returned.
            assertEquals(2, response.getData().getAnswers().size());
            assertEquals("Hello, world", response.getData().getAnswers().get(0).getContent());
            assertEquals("Second answer", response.getData().getAnswers().get(1).getContent());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void the_callback_sees_frames_as_they_arrive() throws Exception {
        HttpServer server = serve(exchange -> {
            try {
                OutputStream out = startNdjson(exchange);
                write(out, FRAMES[0]);
                Thread.sleep(400);
                write(out, FRAMES[1]);
                Thread.sleep(400);
                write(out, terminalFrame(NextAction.TERMINATE));
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        });
        try {
            RequestData request = RequestData.create("r", "f", "hi");
            RequestHandler handler = new RequestHandler(url(server));
            List<Long> arrivedAt = new CopyOnWriteArrayList<>();
            StringBuilder live = new StringBuilder();
            Response response = handler.req(request, 5000, chunk -> {
                arrivedAt.add(System.nanoTime());
                live.append(chunk.getContent());
            });

            assertEquals("Hello, world", live.toString());
            // The two deltas are 400 ms apart on the wire. Only the deltas reach
            // the callback, so that is the whole spread; a buffered read would
            // deliver both in one burst and show almost none.
            assertEquals(2, arrivedAt.size());
            long spreadMillis = (arrivedAt.get(1) - arrivedAt.get(0)) / 1_000_000;
            assertTrue(spreadMillis >= 300,
                    "the deltas arrived " + spreadMillis + " ms apart, they were buffered");

            // The terminal frame is parsed even when a callback consumed the deltas.
            assertEquals(200, response.getStatus());
            assertEquals(NextAction.TERMINATE, response.getData().getNextAction());
            // The streamed text was already handed over; it is not repeated here.
            assertTrue(response.getData().getAnswers().isEmpty());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void an_idle_stream_fails_instead_of_hanging() throws Exception {
        HttpServer server = serve(exchange -> {
            try {
                OutputStream out = startNdjson(exchange);
                write(out, FRAMES[0]);
                // Never sends the terminal frame: the answer stalls mid-flight.
                Thread.sleep(10_000);
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        });
        try {
            RequestData request = RequestData.create("r", "f", "hi");
            RequestHandler handler = new RequestHandler(url(server));
            handler.setIdleTimeoutMillis(300);

            AtomicLong startedAt = new AtomicLong(System.nanoTime());
            IOException e = assertThrows(IOException.class,
                    () -> handler.req(request, 5000, chunk -> { }));
            long tookMillis = (System.nanoTime() - startedAt.get()) / 1_000_000;
            // The request timeout does not cover the body, so nothing but the
            // watchdog ends this.
            assertTrue(tookMillis < 5_000, "gave up only after " + tookMillis + " ms");
            assertTrue(e.getMessage().contains("No frame received"),
                    "unhelpful message: " + e.getMessage());
        } finally {
            server.stop(0);
        }
    }
}
