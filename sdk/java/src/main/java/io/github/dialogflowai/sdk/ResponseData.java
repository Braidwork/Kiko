package io.github.braidwork.kiko.sdk;

import lombok.Data;

import java.util.List;

@Data
public class ResponseData {
    private String sessionId;
    private List<Answer> answers;
    private List<CollectData> collectData;
    private NextAction nextAction;
    private ExtraData extraData;
}
