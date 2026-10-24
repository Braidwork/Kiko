package io.github.braidwork.kiko.sdk;

import lombok.Data;

@Data
public class RequestData {
    private String robotId;
    private String mainFlowId;
    private String sessionId;
    private UserInputResult userInputResult;
    private String userInput;
    private ImportVariable[] importVariables;
    private String userInputIntent;
    /**
     * Asks the server to push the answer as it is produced instead of returning
     * it in one document. A primitive on purpose: it is always serialized, so
     * the request says which of the two it wants rather than leaving it implied.
     */
    private boolean stream;

    public static RequestData create(String robotId, String mainFlowId) {
        RequestData requestData = new RequestData();
        requestData.setRobotId(robotId);
        requestData.setMainFlowId(mainFlowId);
        return requestData;
    }

    public static RequestData create(String robotId, String mainFlowId, String userInput) {
        RequestData requestData = create(robotId, mainFlowId);
        requestData.setUserInput(userInput);
        return requestData;
    }
}
