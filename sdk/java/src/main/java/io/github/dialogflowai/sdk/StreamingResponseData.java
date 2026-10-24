package io.github.braidwork.kiko.sdk;

import lombok.Data;

/**
 * One frame of a streamed answer, as sent by the server on an
 * {@code application/x-ndjson} response body: one JSON document per line.
 *
 * <p>A frame with a {@link #getContentSeq() contentSeq} carries a piece of an
 * answer — append every frame sharing a sequence number, in arrival order, to
 * get that answer's text.
 *
 * <p>A {@code null} contentSeq marks the <em>terminal</em> frame, the last one on
 * the stream. Its {@link #getContent() content} is not text but the whole
 * {@code {status, data, err}} envelope a non-streaming request would return as a
 * document, which is why a caller that consumes frames never loses the final
 * {@code nextAction} or {@code collectData}.
 */
@Data
public class StreamingResponseData {
    private Integer contentSeq;
    private String content;
}
